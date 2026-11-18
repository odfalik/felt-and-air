// Audio Unit (v2) instrument wrapping the engine, written against the plain plug-in
// interface so that it builds with the Command Line Tools alone.
#include <AudioToolbox/AudioToolbox.h>
#include <AudioUnit/AudioUnit.h>
#include <CoreFoundation/CoreFoundation.h>
#include <atomic>
#include <cmath>
#include <cstring>
#include <memory>
#include <pthread.h>
#include <vector>
#include "Engine.h"
#include "Parameters.h"
#include <condition_variable>
#include <mutex>
#include <thread>

using namespace pp;

namespace {

// Parameter numbers: 0 is the volume, then the sliders of the shared table, the switches,
// and the positions of piano and listener on the floor plan.
const UInt32 kParamVolume = 0, kFirstKnob = 1, kFirstToggle = 100, kFirstPlan = 200;
const char* const kPlanNames[4] = {"Piano position, along the keyboard", "Piano position, along the strings", "Listener position, along the keyboard", "Listener position, along the strings"};

bool isParameter(UInt32 id) {
    return id == kParamVolume || (id >= kFirstKnob && id < kFirstKnob + (UInt32)kKnobCount) || (id >= kFirstToggle && id < kFirstToggle + (UInt32)kToggleCount) ||
           (id >= kFirstPlan && id < kFirstPlan + 4);
}
double readOption(const PPOptions& o, UInt32 id) {
    if (id >= kFirstPlan) return o.*(kPlanFields[id - kFirstPlan]);
    if (id >= kFirstToggle) return o.*(kToggles[id - kFirstToggle].field) ? 1.0 : 0.0;
    return o.*(kKnobs[id - kFirstKnob].field);
}
void writeOption(PPOptions& o, UInt32 id, double v) {
    if (id >= kFirstPlan) o.*(kPlanFields[id - kFirstPlan]) = std::fmin(30.0, std::fmax(0.2, v));
    else if (id >= kFirstToggle) o.*(kToggles[id - kFirstToggle].field) = v >= 0.5;
    else { const Knob& k = kKnobs[id - kFirstKnob]; o.*(k.field) = std::fmin(k.hi, std::fmax(k.lo, v)); }
}

struct Listener { AudioUnitPropertyID id; AudioUnitPropertyListenerProc proc; void* user; };
struct Notify { AURenderCallback proc; void* user; };
struct MidiIn { UInt32 offset; int type, a, b; };

struct Unit {
    AudioComponentPlugInInterface iface;  // must be first
    AudioComponentInstance instance = nullptr;
    std::unique_ptr<Piano> piano;
    // Changing a parameter means building a new instrument, which takes a fraction of a second.
    // A worker thread does that; the render thread only swaps pointers.
    GrandOptions options;
    std::mutex optionsLock;
    std::condition_variable optionsChanged;
    bool dirty = false, quit = false;
    std::thread builder;
    std::atomic<Piano*> pending{nullptr}, retired{nullptr};

    Piano* build(const GrandOptions& o) {
        Piano* p = new Piano(o, sampleRate, 1.5);
        if (!p->ok()) { delete p; return nullptr; }
        p->setHelperThreads(3);
        return p;
    }
    void startBuilder() {
        quit = false;
        builder = std::thread([this] {
            std::unique_lock<std::mutex> lk(optionsLock);
            for (;;) {
                optionsChanged.wait(lk, [this] { return dirty || quit; });
                if (quit) return;
                // Let a burst of changes (a dragged slider) settle into one rebuild.
                while (optionsChanged.wait_for(lk, std::chrono::milliseconds(150), [this] { return quit; }) == false && dirty) {
                    dirty = false;
                    GrandOptions o = options;
                    lk.unlock();
                    delete retired.exchange(nullptr);
                    Piano* fresh = build(o);
                    delete pending.exchange(fresh);
                    lk.lock();
                }
                if (quit) return;
            }
        });
    }
    void stopBuilder() {
        if (!builder.joinable()) return;
        { std::lock_guard<std::mutex> g(optionsLock); quit = true; }
        optionsChanged.notify_all();
        builder.join();
        delete pending.exchange(nullptr);
        delete retired.exchange(nullptr);
    }
    void optionsTouched() {
        { std::lock_guard<std::mutex> g(optionsLock); dirty = true; }
        optionsChanged.notify_all();
    }
    double sampleRate = 48000;
    UInt32 maxFrames = 1156;
    bool initialized = false;
    std::atomic<float> volume{0.8f};
    std::vector<float> l, r;
    std::vector<Listener> listeners;
    std::vector<Notify> notifies;
    std::vector<MidiIn> midi;  // events for the next slice, in arrival order
    CFStringRef presetName = CFSTR("Reference grand");
    OSStatus lastError = noErr;

    AudioStreamBasicDescription format() const {
        AudioStreamBasicDescription f = {};
        f.mSampleRate = sampleRate;
        f.mFormatID = kAudioFormatLinearPCM;
        f.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagIsNonInterleaved;
        f.mChannelsPerFrame = 2; f.mBitsPerChannel = 32; f.mBytesPerFrame = 4; f.mBytesPerPacket = 4; f.mFramesPerPacket = 1;
        return f;
    }
    void changed(AudioUnitPropertyID id, AudioUnitScope scope, AudioUnitElement elem) {
        for (const Listener& x : listeners)
            if (x.id == id) x.proc(x.user, instance, id, scope, elem);
    }
    void apply(const MidiIn& m) {
        if (!piano) return;
        if (m.type == 1) piano->noteOn(m.a, midiVelocityToHammerSpeed(m.b));
        else if (m.type == 0) piano->noteOff(m.a, m.b > 0 ? m.b / 64.0 : 1.0);
        else if (m.type == 2) piano->setPedal(m.a / 127.0);
        else if (m.type == 3) { for (int k = 21; k <= 108; k++) piano->noteOff(k); piano->setPedal(0); }
        else if (m.type == 4) piano->setSoftPedal(m.a >= 64);
        else if (m.type == 5) piano->setSostenuto(m.a >= 64);
    }
};

bool rateSupported(double r) { return r >= 44100.0 && r <= 96000.0; }

OSStatus Open(void* self, AudioComponentInstance inst) { ((Unit*)self)->instance = inst; return noErr; }
OSStatus Close(void* self) { ((Unit*)self)->stopBuilder(); delete (Unit*)self; return noErr; }

OSStatus Initialize(void* self) {
    Unit* u = (Unit*)self;
    if (u->initialized) return noErr;
    u->piano.reset(u->build(u->options));
    if (!u->piano) return kAudioUnitErr_FailedInitialization;
    u->startBuilder();
    u->l.assign(u->maxFrames, 0);
    u->r.assign(u->maxFrames, 0);
    u->midi.clear();
    u->midi.reserve(4096);
    u->initialized = true;
    return noErr;
}

OSStatus Uninitialize(void* self) {
    Unit* u = (Unit*)self;
    u->stopBuilder();
    u->piano.reset();
    u->initialized = false;
    return noErr;
}

// Size and writability of each property this unit answers.
OSStatus Info(Unit* u, AudioUnitPropertyID id, AudioUnitScope scope, AudioUnitElement elem, UInt32& size, Boolean& writable) {
    (void)u;
    writable = false;
    switch (id) {
        case kAudioUnitProperty_ClassInfo: if (scope != kAudioUnitScope_Global) return kAudioUnitErr_InvalidScope; size = sizeof(CFPropertyListRef); writable = true; return noErr;
        case kAudioUnitProperty_SampleRate: if (scope == kAudioUnitScope_Input) return kAudioUnitErr_InvalidScope; if (elem != 0) return kAudioUnitErr_InvalidElement; size = sizeof(Float64); writable = true; return noErr;
        case kAudioUnitProperty_ParameterList: size = scope == kAudioUnitScope_Global ? sizeof(AudioUnitParameterID) * (1 + kKnobCount + kToggleCount + 4) : 0; return noErr;
        case kAudioUnitProperty_ParameterInfo: if (scope != kAudioUnitScope_Global || !isParameter(elem)) return kAudioUnitErr_InvalidParameter; size = sizeof(AudioUnitParameterInfo); return noErr;
        case kAudioUnitProperty_StreamFormat: if (scope != kAudioUnitScope_Output) return kAudioUnitErr_InvalidScope; if (elem != 0) return kAudioUnitErr_InvalidElement; size = sizeof(AudioStreamBasicDescription); writable = true; return noErr;
        case kAudioUnitProperty_ElementCount: size = sizeof(UInt32); return noErr;
        case kAudioUnitProperty_Latency: if (scope != kAudioUnitScope_Global) return kAudioUnitErr_InvalidScope; size = sizeof(Float64); return noErr;
        case kAudioUnitProperty_TailTime: if (scope != kAudioUnitScope_Global) return kAudioUnitErr_InvalidScope; size = sizeof(Float64); return noErr;
        case kAudioUnitProperty_SupportedNumChannels: if (scope != kAudioUnitScope_Global) return kAudioUnitErr_InvalidScope; size = sizeof(AUChannelInfo); return noErr;
        case kAudioUnitProperty_MaximumFramesPerSlice: if (scope != kAudioUnitScope_Global) return kAudioUnitErr_InvalidScope; size = sizeof(UInt32); writable = true; return noErr;
        case kAudioUnitProperty_LastRenderError: if (scope != kAudioUnitScope_Global) return kAudioUnitErr_InvalidScope; size = sizeof(OSStatus); return noErr;
        case kAudioUnitProperty_PresentPreset: if (scope != kAudioUnitScope_Global) return kAudioUnitErr_InvalidScope; size = sizeof(AUPreset); writable = true; return noErr;
        case kMusicDeviceProperty_InstrumentCount: if (scope != kAudioUnitScope_Global) return kAudioUnitErr_InvalidScope; size = sizeof(UInt32); return noErr;
        default: return kAudioUnitErr_InvalidProperty;
    }
}

OSStatus GetPropertyInfo(void* self, AudioUnitPropertyID id, AudioUnitScope scope, AudioUnitElement elem, UInt32* outSize, Boolean* outWritable) {
    UInt32 size = 0; Boolean w = false;
    OSStatus err = Info((Unit*)self, id, scope, elem, size, w);
    if (err) return err;
    if (outSize) *outSize = size;
    if (outWritable) *outWritable = w;
    return noErr;
}

CFNumberRef num(SInt32 v) { return CFNumberCreate(nullptr, kCFNumberSInt32Type, &v); }

OSStatus GetProperty(void* self, AudioUnitPropertyID id, AudioUnitScope scope, AudioUnitElement elem, void* out, UInt32* ioSize) {
    Unit* u = (Unit*)self;
    UInt32 size = 0; Boolean w = false;
    OSStatus err = Info(u, id, scope, elem, size, w);
    if (err) return err;
    if (!ioSize) return kAudio_ParamError;
    if (!out) { *ioSize = size; return noErr; }
    if (*ioSize < size) return kAudioUnitErr_InvalidPropertyValue;
    *ioSize = size;
    switch (id) {
        case kAudioUnitProperty_ClassInfo: {
            CFMutableDictionaryRef d = CFDictionaryCreateMutable(nullptr, 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
            auto put = [&](CFStringRef k, SInt32 v) { CFNumberRef n = num(v); CFDictionarySetValue(d, k, n); CFRelease(n); };
            put(CFSTR(kAUPresetVersionKey), 0);
            put(CFSTR(kAUPresetTypeKey), kAudioUnitType_MusicDevice);
            put(CFSTR(kAUPresetSubtypeKey), 'PhPn');
            put(CFSTR(kAUPresetManufacturerKey), 'Odfk');
            CFDictionarySetValue(d, CFSTR(kAUPresetNameKey), u->presetName);
            // The saved state is the volume followed by the instrument's settings.
            std::vector<UInt8> blob(sizeof(Float32) + sizeof(PPOptions));
            Float32 vol = u->volume.load();
            memcpy(blob.data(), &vol, sizeof(vol));
            { std::lock_guard<std::mutex> g(u->optionsLock); memcpy(blob.data() + sizeof(vol), static_cast<PPOptions*>(&u->options), sizeof(PPOptions)); }
            CFDataRef data = CFDataCreate(nullptr, blob.data(), (CFIndex)blob.size());
            CFDictionarySetValue(d, CFSTR(kAUPresetDataKey), data);
            CFRelease(data);
            *(CFPropertyListRef*)out = d;
            return noErr;
        }
        case kAudioUnitProperty_SampleRate: *(Float64*)out = u->sampleRate; return noErr;
        case kAudioUnitProperty_ParameterList: {
            if (!size) return noErr;
            AudioUnitParameterID* ids = (AudioUnitParameterID*)out;
            *ids++ = kParamVolume;
            for (int i = 0; i < kKnobCount; i++) *ids++ = kFirstKnob + i;
            for (int i = 0; i < kToggleCount; i++) *ids++ = kFirstToggle + i;
            for (int i = 0; i < 4; i++) *ids++ = kFirstPlan + i;
            return noErr;
        }
        case kAudioUnitProperty_ParameterInfo: {
            AudioUnitParameterInfo info = {};
            info.flags = kAudioUnitParameterFlag_IsReadable | kAudioUnitParameterFlag_IsWritable | kAudioUnitParameterFlag_HasCFNameString | kAudioUnitParameterFlag_CFNameRelease;
            const char* name = "Volume";
            info.unit = kAudioUnitParameterUnit_Generic;
            GrandOptions reference;
            if (elem == kParamVolume) {
                info.unit = kAudioUnitParameterUnit_LinearGain;
                info.minValue = 0; info.maxValue = 3; info.defaultValue = 0.8f;
            } else if (elem >= kFirstPlan) {
                name = kPlanNames[elem - kFirstPlan];
                info.unit = kAudioUnitParameterUnit_Meters;
                info.minValue = 0.2f; info.maxValue = 30;
                info.defaultValue = (float)readOption(reference, elem);
            } else if (elem >= kFirstToggle) {
                name = kToggles[elem - kFirstToggle].name;
                info.unit = kAudioUnitParameterUnit_Boolean;
                info.minValue = 0; info.maxValue = 1;
                info.defaultValue = (float)readOption(reference, elem);
            } else {
                const Knob& k = kKnobs[elem - kFirstKnob];
                name = k.name;
                info.minValue = (float)k.lo; info.maxValue = (float)k.hi;
                info.defaultValue = (float)readOption(reference, elem);
                if (k.logScale) info.flags |= kAudioUnitParameterFlag_DisplayLogarithmic;
            }
            info.cfNameString = CFStringCreateWithCString(nullptr, name, kCFStringEncodingUTF8);
            strlcpy(info.name, name, sizeof(info.name));
            *(AudioUnitParameterInfo*)out = info;
            return noErr;
        }
        case kAudioUnitProperty_StreamFormat: *(AudioStreamBasicDescription*)out = u->format(); return noErr;
        case kAudioUnitProperty_ElementCount: *(UInt32*)out = scope == kAudioUnitScope_Input ? 0 : 1; return noErr;
        case kAudioUnitProperty_Latency: *(Float64*)out = 0; return noErr;
        case kAudioUnitProperty_TailTime: *(Float64*)out = 8.0; return noErr;
        case kAudioUnitProperty_SupportedNumChannels: { AUChannelInfo ci = {0, 2}; *(AUChannelInfo*)out = ci; return noErr; }
        case kAudioUnitProperty_MaximumFramesPerSlice: *(UInt32*)out = u->maxFrames; return noErr;
        case kAudioUnitProperty_LastRenderError: *(OSStatus*)out = u->lastError; u->lastError = noErr; return noErr;
        case kAudioUnitProperty_PresentPreset: { AUPreset p = {-1, u->presetName}; CFRetain(u->presetName); *(AUPreset*)out = p; return noErr; }
        case kMusicDeviceProperty_InstrumentCount: *(UInt32*)out = 0; return noErr;
    }
    return kAudioUnitErr_InvalidProperty;
}

OSStatus SetProperty(void* self, AudioUnitPropertyID id, AudioUnitScope scope, AudioUnitElement elem, const void* in, UInt32 inSize) {
    Unit* u = (Unit*)self;
    UInt32 size = 0; Boolean w = false;
    OSStatus err = Info(u, id, scope, elem, size, w);
    if (err) return err;
    if (!w) return kAudioUnitErr_PropertyNotWritable;
    if (!in || inSize < size) return kAudioUnitErr_InvalidPropertyValue;
    switch (id) {
        case kAudioUnitProperty_ClassInfo: {
            CFPropertyListRef pl = *(CFPropertyListRef*)in;
            if (!pl || CFGetTypeID(pl) != CFDictionaryGetTypeID()) return kAudioUnitErr_InvalidPropertyValue;
            CFDictionaryRef d = (CFDictionaryRef)pl;
            auto match = [&](CFStringRef key, SInt32 want) {
                CFNumberRef n = (CFNumberRef)CFDictionaryGetValue(d, key);
                SInt32 v = 0;
                return n && CFGetTypeID(n) == CFNumberGetTypeID() && CFNumberGetValue(n, kCFNumberSInt32Type, &v) && v == want;
            };
            if (!match(CFSTR(kAUPresetTypeKey), kAudioUnitType_MusicDevice) || !match(CFSTR(kAUPresetSubtypeKey), 'PhPn') ||
                !match(CFSTR(kAUPresetManufacturerKey), 'Odfk'))
                return kAudioUnitErr_InvalidPropertyValue;
            CFDataRef data = (CFDataRef)CFDictionaryGetValue(d, CFSTR(kAUPresetDataKey));
            if (data && CFGetTypeID(data) == CFDataGetTypeID() && CFDataGetLength(data) == (CFIndex)(sizeof(Float32) + sizeof(PPOptions))) {
                Float32 vol;
                CFDataGetBytes(data, CFRangeMake(0, sizeof(vol)), (UInt8*)&vol);
                u->volume = vol;
                PPOptions saved;
                CFDataGetBytes(data, CFRangeMake(sizeof(vol), sizeof(saved)), (UInt8*)&saved);
                saved.first_key = 1; saved.last_key = 88;
                { std::lock_guard<std::mutex> g(u->optionsLock); static_cast<PPOptions&>(u->options) = saved; }
                if (u->initialized) u->optionsTouched();
            }
            return noErr;
        }
        case kAudioUnitProperty_SampleRate: {
            Float64 r = *(const Float64*)in;
            if (u->initialized) return kAudioUnitErr_Initialized;
            if (!rateSupported(r)) return kAudioUnitErr_FormatNotSupported;
            u->sampleRate = r;
            u->changed(kAudioUnitProperty_SampleRate, scope, elem);
            return noErr;
        }
        case kAudioUnitProperty_StreamFormat: {
            const AudioStreamBasicDescription& f = *(const AudioStreamBasicDescription*)in;
            AudioStreamBasicDescription mine = u->format();
            if (f.mFormatID != kAudioFormatLinearPCM || f.mChannelsPerFrame != 2 || f.mBitsPerChannel != 32 ||
                (f.mFormatFlags & kAudioFormatFlagIsFloat) == 0 || (f.mFormatFlags & kAudioFormatFlagIsNonInterleaved) == 0 ||
                !rateSupported(f.mSampleRate))
                return kAudioUnitErr_FormatNotSupported;
            if (f.mSampleRate != mine.mSampleRate) {
                if (u->initialized) return kAudioUnitErr_Initialized;
                u->sampleRate = f.mSampleRate;
            }
            u->changed(kAudioUnitProperty_StreamFormat, scope, elem);
            return noErr;
        }
        case kAudioUnitProperty_MaximumFramesPerSlice: {
            if (u->initialized) return kAudioUnitErr_Initialized;
            u->maxFrames = *(const UInt32*)in;
            u->changed(kAudioUnitProperty_MaximumFramesPerSlice, scope, elem);
            return noErr;
        }
        case kAudioUnitProperty_PresentPreset: return noErr;
    }
    return kAudioUnitErr_InvalidProperty;
}

OSStatus AddPropertyListener(void* self, AudioUnitPropertyID id, AudioUnitPropertyListenerProc proc, void* user) {
    ((Unit*)self)->listeners.push_back({id, proc, user});
    return noErr;
}
OSStatus RemovePropertyListener(void* self, AudioUnitPropertyID id, AudioUnitPropertyListenerProc proc) {
    auto& v = ((Unit*)self)->listeners;
    for (size_t i = v.size(); i-- > 0;) if (v[i].id == id && v[i].proc == proc) v.erase(v.begin() + i);
    return noErr;
}
OSStatus RemovePropertyListenerWithUserData(void* self, AudioUnitPropertyID id, AudioUnitPropertyListenerProc proc, void* user) {
    auto& v = ((Unit*)self)->listeners;
    for (size_t i = v.size(); i-- > 0;) if (v[i].id == id && v[i].proc == proc && v[i].user == user) v.erase(v.begin() + i);
    return noErr;
}
OSStatus AddRenderNotify(void* self, AURenderCallback proc, void* user) { ((Unit*)self)->notifies.push_back({proc, user}); return noErr; }
OSStatus RemoveRenderNotify(void* self, AURenderCallback proc, void* user) {
    auto& v = ((Unit*)self)->notifies;
    for (size_t i = v.size(); i-- > 0;) if (v[i].proc == proc && v[i].user == user) v.erase(v.begin() + i);
    return noErr;
}

OSStatus GetParameter(void* self, AudioUnitParameterID id, AudioUnitScope scope, AudioUnitElement, AudioUnitParameterValue* out) {
    if (scope != kAudioUnitScope_Global) return kAudioUnitErr_InvalidScope;
    Unit* u = (Unit*)self;
    if (!isParameter(id)) return kAudioUnitErr_InvalidParameter;
    if (id == kParamVolume) { *out = u->volume.load(); return noErr; }
    std::lock_guard<std::mutex> g(u->optionsLock);
    *out = (AudioUnitParameterValue)readOption(u->options, id);
    return noErr;
}
OSStatus SetParameter(void* self, AudioUnitParameterID id, AudioUnitScope scope, AudioUnitElement, AudioUnitParameterValue v, UInt32) {
    if (scope != kAudioUnitScope_Global) return kAudioUnitErr_InvalidScope;
    Unit* u = (Unit*)self;
    if (!isParameter(id)) return kAudioUnitErr_InvalidParameter;
    if (id == kParamVolume) { u->volume = std::fmin(3.0f, std::fmax(0.0f, v)); return noErr; }
    // Hosts may call this from the render thread; the lock is held only for a few stores.
    { std::lock_guard<std::mutex> g(u->optionsLock); writeOption(u->options, id, v); u->dirty = true; }
    u->optionsChanged.notify_all();
    return noErr;
}
OSStatus ScheduleParameters(void* self, const AudioUnitParameterEvent* ev, UInt32 n) {
    for (UInt32 i = 0; i < n; i++)
        if (ev[i].eventType == kParameterEvent_Immediate) SetParameter(self, ev[i].parameter, ev[i].scope, ev[i].element, ev[i].eventValues.immediate.value, 0);
    return noErr;
}

OSStatus Reset(void* self, AudioUnitScope, AudioUnitElement) {
    Unit* u = (Unit*)self;
    u->midi.clear();
    u->apply({0, 3, 0, 0});
    return noErr;
}

OSStatus MIDIEvent(void* self, UInt32 status, UInt32 d1, UInt32 d2, UInt32 offset) {
    Unit* u = (Unit*)self;
    if (!u->initialized) return kAudioUnitErr_Uninitialized;
    int kind = status & 0xF0;
    MidiIn m{offset, -1, (int)d1, (int)d2};
    if (kind == 0x90 && d2 > 0) m.type = 1;
    else if (kind == 0x80 || kind == 0x90) { m.type = 0; if (kind == 0x90) m.b = 0; }
    else if (kind == 0xB0 && d1 == 64) { m.type = 2; m.a = (int)d2; }
    else if (kind == 0xB0 && (d1 == 120 || d1 == 123)) m.type = 3;
    else if (kind == 0xB0 && d1 == 67) { m.type = 4; m.a = (int)d2; }
    else if (kind == 0xB0 && d1 == 66) { m.type = 5; m.a = (int)d2; }
    if (m.type >= 0 && u->midi.size() < u->midi.capacity()) u->midi.push_back(m);
    return noErr;
}
OSStatus SysEx(void*, const UInt8*, UInt32) { return noErr; }

OSStatus Render(void* self, AudioUnitRenderActionFlags* ioFlags, const AudioTimeStamp* ts, UInt32 bus, UInt32 frames, AudioBufferList* io) {
    Unit* u = (Unit*)self;
    if (!u->initialized) return kAudioUnitErr_Uninitialized;
    if (bus != 0) return kAudioUnitErr_InvalidElement;
    if (frames > u->maxFrames) return u->lastError = kAudioUnitErr_TooManyFramesToProcess;
    if (!io || io->mNumberBuffers < 1) return kAudio_ParamError;
    if (Piano* fresh = u->pending.exchange(nullptr)) {  // a rebuilt instrument is ready: swap it in
        delete u->retired.exchange(u->piano.release());  // normally empty; the builder frees the old one
        u->piano.reset(fresh);
    }
    AudioUnitRenderActionFlags flags = ioFlags ? *ioFlags : 0;
    for (const Notify& nfy : u->notifies) { AudioUnitRenderActionFlags f = flags | kAudioUnitRenderAction_PreRender; nfy.proc(nfy.user, &f, ts, bus, frames, io); }

    // Render in pieces so that each event lands on its own sample.
    UInt32 done = 0;
    size_t next = 0;
    while (done < frames) {
        UInt32 until = frames;
        // Events are few per slice; a linear scan for the earliest pending one is enough.
        for (;;) {
            size_t best = u->midi.size();
            for (size_t i = 0; i < u->midi.size(); i++)
                if (u->midi[i].type >= 0 && (best == u->midi.size() || u->midi[i].offset < u->midi[best].offset)) best = i;
            if (best == u->midi.size()) break;
            if (u->midi[best].offset <= done) { u->apply(u->midi[best]); u->midi[best].type = -1; continue; }
            until = u->midi[best].offset < frames ? u->midi[best].offset : frames;
            break;
        }
        u->piano->process(u->l.data() + done, u->r.data() + done, (int)(until - done));
        done = until;
    }
    (void)next;
    for (MidiIn& m : u->midi) if (m.type >= 0) u->apply(m);  // anything stamped past the slice
    u->midi.clear();

    const float vol = u->volume.load();
    for (UInt32 b = 0; b < io->mNumberBuffers && b < 2; b++) {
        const float* src = b == 0 ? u->l.data() : u->r.data();
        if (!io->mBuffers[b].mData) io->mBuffers[b].mData = (void*)src;  // host asked us to supply the memory
        float* dst = (float*)io->mBuffers[b].mData;
        if (dst == src) { for (UInt32 i = 0; i < frames; i++) u->l[i] = u->l[i]; }
        for (UInt32 i = 0; i < frames; i++) dst[i] = std::tanh(src[i] * vol);
        io->mBuffers[b].mDataByteSize = frames * sizeof(float);
    }
    for (const Notify& nfy : u->notifies) { AudioUnitRenderActionFlags f = flags | kAudioUnitRenderAction_PostRender; nfy.proc(nfy.user, &f, ts, bus, frames, io); }
    return noErr;
}

AudioComponentMethod Lookup(SInt16 selector) {
    switch (selector) {
        case kAudioUnitInitializeSelect: return (AudioComponentMethod)Initialize;
        case kAudioUnitUninitializeSelect: return (AudioComponentMethod)Uninitialize;
        case kAudioUnitGetPropertyInfoSelect: return (AudioComponentMethod)GetPropertyInfo;
        case kAudioUnitGetPropertySelect: return (AudioComponentMethod)GetProperty;
        case kAudioUnitSetPropertySelect: return (AudioComponentMethod)SetProperty;
        case kAudioUnitAddPropertyListenerSelect: return (AudioComponentMethod)AddPropertyListener;
        case kAudioUnitRemovePropertyListenerSelect: return (AudioComponentMethod)RemovePropertyListener;
        case kAudioUnitRemovePropertyListenerWithUserDataSelect: return (AudioComponentMethod)RemovePropertyListenerWithUserData;
        case kAudioUnitAddRenderNotifySelect: return (AudioComponentMethod)AddRenderNotify;
        case kAudioUnitRemoveRenderNotifySelect: return (AudioComponentMethod)RemoveRenderNotify;
        case kAudioUnitGetParameterSelect: return (AudioComponentMethod)GetParameter;
        case kAudioUnitSetParameterSelect: return (AudioComponentMethod)SetParameter;
        case kAudioUnitScheduleParametersSelect: return (AudioComponentMethod)ScheduleParameters;
        case kAudioUnitRenderSelect: return (AudioComponentMethod)Render;
        case kAudioUnitResetSelect: return (AudioComponentMethod)Reset;
        case kMusicDeviceMIDIEventSelect: return (AudioComponentMethod)MIDIEvent;
        case kMusicDeviceSysExSelect: return (AudioComponentMethod)SysEx;
        default: return nullptr;
    }
}

}  // namespace

extern "C" __attribute__((visibility("default"))) void* PhysicalPianoFactory(const AudioComponentDescription*) {
    Unit* u = new Unit;
    u->iface.Open = Open;
    u->iface.Close = Close;
    u->iface.Lookup = Lookup;
    u->iface.reserved = nullptr;
    return u;
}
