// Loads the Audio Unit bundle in this process, without installing it, and drives it the way
// a host does: set the format, initialise, send notes, pull audio.
#include <AudioToolbox/AudioToolbox.h>
#include <cmath>
#include <cstdio>
#include <dlfcn.h>
#include <unistd.h>
#include <vector>

int main(int argc, char** argv) {
    const char* path = argc > 1 ? argv[1] : "build/PhysicalPiano.component/Contents/MacOS/PhysicalPiano";
    void* lib = dlopen(path, RTLD_NOW);
    if (!lib) { std::printf("cannot load %s: %s\n", path, dlerror()); return 1; }
    auto factory = (AudioComponentFactoryFunction)dlsym(lib, "PhysicalPianoFactory");
    if (!factory) { std::printf("factory not found\n"); return 1; }
    AudioComponentDescription desc = {kAudioUnitType_MusicDevice, 'PhPn', 'Odfk', 0, 0};
    AudioComponent comp = AudioComponentRegister(&desc, CFSTR("odfalik: Physical Piano"), 0x10000, factory);
    AudioUnit au = nullptr;
    if (!comp || AudioComponentInstanceNew(comp, &au) != noErr) { std::printf("cannot instantiate\n"); return 1; }

    int failures = 0;
    auto check = [&](bool ok, const char* what) { std::printf("  [%s] %s\n", ok ? "ok" : "FAIL", what); failures += !ok; };
    AudioStreamBasicDescription f = {};
    UInt32 size = sizeof(f);
    check(AudioUnitGetProperty(au, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output, 0, &f, &size) == noErr && f.mChannelsPerFrame == 2, "reports a stereo float output format");
    f.mSampleRate = 44100;
    check(AudioUnitSetProperty(au, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output, 0, &f, sizeof(f)) == noErr, "accepts 44.1 kHz");
    f.mSampleRate = 192000;
    check(AudioUnitSetProperty(au, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output, 0, &f, sizeof(f)) != noErr, "refuses 192 kHz");
    f.mSampleRate = 48000;
    AudioUnitSetProperty(au, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output, 0, &f, sizeof(f));
    UInt32 maxFrames = 512;
    AudioUnitSetProperty(au, kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitScope_Global, 0, &maxFrames, sizeof(maxFrames));
    check(AudioUnitInitialize(au) == noErr, "initialises at 48 kHz");

    std::vector<float> l(512), r(512);
    AudioBufferList* io = (AudioBufferList*)calloc(1, sizeof(AudioBufferList) + sizeof(AudioBuffer));
    io->mNumberBuffers = 2;
    AudioTimeStamp ts = {};
    ts.mFlags = kAudioTimeStampSampleTimeValid;
    double peak = 0, early = 0, late = 0;
    bool finite = true;
    for (int block = 0; block < 280; block++) {
        if (block == 2) { MusicDeviceMIDIEvent(au, 0x90, 60, 90, 100); MusicDeviceMIDIEvent(au, 0x90, 64, 80, 300); MusicDeviceMIDIEvent(au, 0xB0, 64, 127, 0); }
        if (block == 150) { MusicDeviceMIDIEvent(au, 0x80, 60, 64, 0); MusicDeviceMIDIEvent(au, 0x80, 64, 64, 0); MusicDeviceMIDIEvent(au, 0xB0, 64, 0, 10); }
        io->mBuffers[0] = {1, 512 * 4, l.data()};
        io->mBuffers[1] = {1, 512 * 4, r.data()};
        AudioUnitRenderActionFlags flags = 0;
        OSStatus err = AudioUnitRender(au, &flags, &ts, 0, 512, io);
        if (err) { std::printf("render error %d\n", (int)err); return 1; }
        ts.mSampleTime += 512;
        for (int i = 0; i < 512; i++) {
            finite = finite && std::isfinite(l[i]);
            peak = std::fmax(peak, std::fabs(l[i]));
            if (block >= 4 && block < 40) early = std::fmax(early, std::fabs(l[i]));
            if (block >= 250) late = std::fmax(late, std::fabs(l[i]));
        }
    }
    check(finite && peak > 0.01 && peak <= 1.0, "notes sent as MIDI produce sound within range");
    check(late < 0.05 * early, "note-off and pedal-up bring it back to quiet");
    std::printf("  peak %.3f, after release %.4f\n", peak, late);
    AudioBufferList* nul = (AudioBufferList*)calloc(1, sizeof(AudioBufferList) + sizeof(AudioBuffer));
    nul->mNumberBuffers = 2;
    AudioUnitRenderActionFlags flags = 0;
    check(AudioUnitRender(au, &flags, &ts, 0, 256, nul) == noErr && nul->mBuffers[0].mData != nullptr, "supplies its own buffers when the host passes none");
    check(AudioUnitRender(au, &flags, &ts, 0, 4096, io) == kAudioUnitErr_TooManyFramesToProcess, "refuses a slice larger than agreed");
    CFPropertyListRef state = nullptr;
    size = sizeof(state);
    check(AudioUnitGetProperty(au, kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global, 0, &state, &size) == noErr && state &&
          AudioUnitSetProperty(au, kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global, 0, &state, sizeof(state)) == noErr, "saves and restores its state");
    UInt32 listSize = 0;
    AudioUnitGetPropertyInfo(au, kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0, &listSize, nullptr);
    int params = (int)(listSize / sizeof(AudioUnitParameterID));
    std::vector<AudioUnitParameterID> ids(params);
    AudioUnitGetProperty(au, kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0, ids.data(), &listSize);
    bool infoOk = params > 30;
    for (AudioUnitParameterID id : ids) {
        AudioUnitParameterInfo info = {};
        UInt32 sz = sizeof(info);
        infoOk = infoOk && AudioUnitGetProperty(au, kAudioUnitProperty_ParameterInfo, kAudioUnitScope_Global, id, &info, &sz) == noErr && info.maxValue > info.minValue;
        if (info.cfNameString) CFRelease(info.cfNameString);
    }
    std::printf("  %d parameters\n", params);
    check(infoOk, "every setting of the instrument is a described parameter");
    // Change one (felt hardness, parameter 8) and give the rebuild time to land.
    AudioUnitSetParameter(au, 8, kAudioUnitScope_Global, 0, 3.0f, 0);
    AudioUnitParameterValue back = 0;
    AudioUnitGetParameter(au, 8, kAudioUnitScope_Global, 0, &back);
    double after = 0;
    for (int block = 0; block < 400; block++) {
        if (block == 300) MusicDeviceMIDIEvent(au, 0x90, 60, 90, 0);
        io->mBuffers[0] = {1, 512 * 4, l.data()};
        io->mBuffers[1] = {1, 512 * 4, r.data()};
        if (block < 300) usleep(2000);
        AudioUnitRender(au, &flags, &ts, 0, 512, io);
        ts.mSampleTime += 512;
        if (block > 300) for (int i = 0; i < 512; i++) after = std::fmax(after, std::fabs(l[i]));
    }
    check(back == 3.0f && after > 0.01 && after <= 1.0, "a parameter change rebuilds the instrument and it still plays");
    AudioUnitUninitialize(au);
    AudioComponentInstanceDispose(au);
    std::printf("%s\n", failures ? "FAILED" : "ALL PASSED");
    return failures ? 1 : 0;
}
