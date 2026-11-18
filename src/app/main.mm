// Standalone instrument: CoreAudio output, CoreMIDI and computer-keyboard input, and a live
// view of the soundboard and strings. Audio stays off until the user turns it on.
#import <AppKit/AppKit.h>
#import <AudioToolbox/AudioToolbox.h>
#import <CoreAudio/CoreAudio.h>
#import <CoreMIDI/CoreMIDI.h>
#include <atomic>
#include <cmath>
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <mach/thread_policy.h>
#include <os/workgroup.h>
#include <memory>
#include <pthread.h>
#include <vector>
#include "Midi.h"
#include "Piano.h"
#include <fstream>
#include <sstream>
#include <string>

using namespace pp;

// Single producer, single consumer queue of control events.
struct Event { int type, a, b; };  // 1 note on (a midi, b velocity), 0 note off, 2 sustain (a), 4 play demo a, 5 stop demo
struct EventQueue {
    static const int N = 1024;
    Event buf[N];
    std::atomic<int> head{0}, tail{0};
    bool push(Event e) {
        int h = head.load(std::memory_order_relaxed), n = (h + 1) % N;
        if (n == tail.load(std::memory_order_acquire)) return false;
        buf[h] = e;
        head.store(n, std::memory_order_release);
        return true;
    }
    bool pop(Event& e) {
        int t = tail.load(std::memory_order_relaxed);
        if (t == head.load(std::memory_order_acquire)) return false;
        e = buf[t];
        tail.store((t + 1) % N, std::memory_order_release);
        return true;
    }
};

struct Engine {
    PianoDesign design;
    std::unique_ptr<Piano> piano;
    Piano::Snapshot snap, snapUI;
    std::atomic_flag snapLock = ATOMIC_FLAG_INIT;
    EventQueue uiEvents, midiEvents;
    std::vector<float> l, r;
    std::atomic<float> volume{0.8f}, load{0}, loadPeak{0}, level{0};
    std::atomic<int> focus{60}, overruns{0}, blocks{0};
    std::atomic<bool> mute{false};  // write silence (used to measure on the real audio thread without sound)
    double sampleRate = 48000, gridScale = 1.5;
    os_workgroup_t workgroup = nullptr;  // the audio device's realtime thread group
    bool sideways = false;
    int listener = 0;  // 0 in the room, 1 at the keyboard, 2 no room
    GrandOptions voicing;  // what the Instrument panel edits
    int framesSinceSnap = 0;

    // Demo recordings, loaded once at start. The audio thread only reads them.
    struct Demo { std::string title, note; std::vector<MidiEvent> events; };
    std::vector<Demo> demos;
    std::atomic<int> demoPlaying{-1};
    std::atomic<float> demoSeconds{0};
    int demoIndex = -1;
    size_t demoNext = 0;
    long demoFrame = 0;

    void loadDemos(const std::string& dir) {
        std::ifstream list(dir + "/demos.txt");
        std::string line;
        while (std::getline(list, line)) {
            std::stringstream ss(line);
            std::string file, title, note;
            std::getline(ss, file, '|'); std::getline(ss, title, '|'); std::getline(ss, note, '|');
            Demo d{title, note, readMidiFile(dir + "/" + file)};
            if (!d.events.empty()) demos.push_back(std::move(d));
        }
    }
    void silence() {
        for (int m = 21; m <= 108; m++) piano->noteOff(m);
        piano->setSustain(false);
    }
    mach_timebase_info_data_t tb;

    void build() {
        GrandOptions g = voicing;
        g.twoPolarisations = sideways;
        g.playerPerspective = listener == 1;
        design = makeGrand(g);
        if (listener == 2) { design.room.enabled = false; design.lid.open = false; }
        demoPlaying = -1; demoIndex = -1;
        PianoOptions o;
        o.sampleRate = sampleRate;
        o.boardGridScale = gridScale;
        piano = std::make_unique<Piano>(design, o);
        // Helpers join the audio thread's workgroup so the system schedules them as one
        // realtime unit instead of preempting them mid-step.
        os_workgroup_t wg = workgroup;
        // A thread must leave the group before it ends, or the system aborts the process.
        static thread_local os_workgroup_join_token_s token;
        static thread_local bool joined = false;
        piano->setHelperThreads(
            3,
            [wg] {
                // Same scheduling class as the audio thread: a realtime thread with a stated
                // period and work share, so it is given a performance core when it is needed.
                mach_timebase_info_data_t t;
                mach_timebase_info(&t);
                auto ticks = [&](double seconds) { return (uint32_t)(seconds * 1e9 * t.denom / t.numer); };
                thread_time_constraint_policy_data_t pol;
                pol.period = ticks(0.0029); pol.computation = ticks(0.0012); pol.constraint = ticks(0.0029); pol.preemptible = 1;
                thread_policy_set(mach_thread_self(), THREAD_TIME_CONSTRAINT_POLICY, (thread_policy_t)&pol, THREAD_TIME_CONSTRAINT_POLICY_COUNT);
                joined = wg && os_workgroup_join(wg, &token) == 0;
            },
            [wg] { if (joined) os_workgroup_leave(wg, &token); joined = false; });
        piano->prepareSnapshot(snap);
        piano->prepareSnapshot(snapUI);
        l.assign(8192, 0); r.assign(8192, 0);
        mach_timebase_info(&tb);
    }
    void handle(const Event& e) {
        if (e.type == 1) { piano->noteOn(e.a, midiVelocityToHammerSpeed(e.b)); focus = e.a; }
        else if (e.type == 0) piano->noteOff(e.a);
        else if (e.type == 2) piano->setPedal(e.a / 127.0);
        else if (e.type == 4 && e.a >= 0 && e.a < (int)demos.size()) { silence(); demoIndex = e.a; demoNext = 0; demoFrame = 0; demoPlaying = e.a; }
        else if (e.type == 5) { silence(); demoIndex = -1; demoPlaying = -1; }
    }
    // Runs on the audio thread: no locks, no allocation.
    void render(float* outL, float* outR, int n) {
        uint64_t t0 = mach_absolute_time();
        Event e;
        while (uiEvents.pop(e)) handle(e);
        while (midiEvents.pop(e)) handle(e);
        n = std::min(n, (int)l.size());
        // Render in pieces so that recorded notes land on their exact sample.
        for (int done = 0; done < n;) {
            int until = n;
            if (demoIndex >= 0) {
                const auto& ev = demos[demoIndex].events;
                while (demoNext < ev.size() && (long)(ev[demoNext].seconds * sampleRate) <= demoFrame) {
                    const MidiEvent& m = ev[demoNext++];
                    handle({m.type, m.a, m.b});
                }
                if (demoNext < ev.size()) until = (int)std::min<long>(n, done + (long)(ev[demoNext].seconds * sampleRate) - demoFrame);
                else if (demoFrame > (long)((ev.back().seconds + 4.0) * sampleRate)) { demoIndex = -1; demoPlaying = -1; }
                until = std::max(until, done + 1);
            }
            piano->process(l.data() + done, r.data() + done, until - done);
            if (demoIndex >= 0) { demoFrame += until - done; demoSeconds = (float)(demoFrame / sampleRate); }
            done = until;
        }
        float vol = volume.load(), pk = 0;
        for (int i = 0; i < n; i++) {
            float a = std::tanh(l[i] * vol), b = std::tanh(r[i] * vol);  // soft ceiling
            outL[i] = a; outR[i] = b;
            if (mute.load(std::memory_order_relaxed)) outL[i] = outR[i] = 0;
            pk = std::max(pk, std::max(std::fabs(a), std::fabs(b)));
        }
        level = std::max(pk, level.load() * 0.9f);
        framesSinceSnap += n;
        if (framesSinceSnap >= sampleRate / 40 && !snapLock.test_and_set(std::memory_order_acquire)) {
            piano->fillSnapshot(snap, focus.load());
            snapLock.clear(std::memory_order_release);
            framesSinceSnap = 0;
        }
        double sec = (double)(mach_absolute_time() - t0) * tb.numer / tb.denom * 1e-9;
        float ld = (float)(sec / (n / sampleRate));
        load = load.load() * 0.95f + ld * 0.05f;
        if (ld > loadPeak.load()) loadPeak = ld;
        if (ld > 1.0f) overruns++;
        blocks++;
    }
};

static Engine gEngine;

static OSStatus renderCallback(void*, AudioUnitRenderActionFlags*, const AudioTimeStamp*, UInt32, UInt32 frames, AudioBufferList* io) {
    float* L = (float*)io->mBuffers[0].mData;
    float* R = io->mNumberBuffers > 1 ? (float*)io->mBuffers[1].mData : L;
    gEngine.render(L, R, (int)frames);
    return noErr;
}

// ------------------------------------------------------------------------------ view

@interface PianoView : NSView
- (void)rebuildMap;
@property(nonatomic) int octave;
@property(nonatomic) int velocity;
@property(nonatomic) BOOL audioOn;
@end

@implementation PianoView {
    std::vector<int> _pixMap;       // board image pixel -> grid storage index, -1 outside
    std::vector<uint8_t> _pixels;
    int _iw, _ih;
    bool _down[128];
    int _mouseNote;
    float _boardScale;
}

- (instancetype)initWithFrame:(NSRect)f {
    if ((self = [super initWithFrame:f])) {
        _octave = 4; _velocity = 80; _mouseNote = -1; _boardScale = 1e-6f;
        memset(_down, 0, sizeof(_down));
        [self rebuildMap];
    }
    return self;
}
- (BOOL)acceptsFirstResponder { return YES; }
- (BOOL)isFlipped { return YES; }

- (void)rebuildMap {
    // Board picture in piano coordinates: keyboard at the bottom, tail at the top.
    _iw = 280; _ih = 300;
    _pixMap.assign(_iw * _ih, -1);
    _pixels.assign(_iw * _ih * 4, 0);
    const Plate& P = gEngine.piano->plate();
    for (int y = 0; y < _ih; y++)
        for (int x = 0; x < _iw; x++) {
            Vec2 p{1.40 * (x + 0.5) / _iw, 1.50 * (1.0 - (y + 0.5) / _ih)};
            Interp it = P.interp(p);
            int best = -1; double bw = 0.2;
            for (int k = 0; k < 4; k++) if (it.w[k] > bw) { bw = it.w[k]; best = it.idx[k]; }
            _pixMap[y * _iw + x] = best;
        }
}

- (NSRect)boardRect { return NSMakeRect(20, 50, 420, 450); }
- (NSRect)keysRect { return NSMakeRect(20, self.bounds.size.height - 110, self.bounds.size.width - 40, 90); }
- (void)clearKeys { memset(_down, 0, sizeof(_down)); }

static bool isBlack(int midi) { int k = midi % 12; return k == 1 || k == 3 || k == 6 || k == 8 || k == 10; }

- (void)drawRect:(NSRect)dirty {
    Engine& E = gEngine;
    // Copy, not swap: the display may redraw several times between two audio snapshots.
    while (E.snapLock.test_and_set(std::memory_order_acquire)) {}
    E.snapUI = E.snap;
    E.snapLock.clear(std::memory_order_release);
    const Piano::Snapshot& S = E.snapUI;

    [[NSColor colorWithWhite:0.09 alpha:1] setFill];
    NSRectFill(self.bounds);
    NSDictionary* small = @{NSFontAttributeName: [NSFont monospacedSystemFontOfSize:11 weight:NSFontWeightRegular],
                            NSForegroundColorAttributeName: [NSColor colorWithWhite:0.75 alpha:1]};
    NSDictionary* title = @{NSFontAttributeName: [NSFont systemFontOfSize:13 weight:NSFontWeightSemibold],
                            NSForegroundColorAttributeName: [NSColor colorWithWhite:0.95 alpha:1]};

    // Soundboard displacement, blue down / orange up, auto-ranged.
    float mx = 1e-9f;
    for (float v : S.board) mx = std::max(mx, std::fabs(v));
    _boardScale = std::max(mx, _boardScale * 0.97f);
    float inv = 1.0f / std::max(_boardScale, 2e-8f);
    for (int i = 0; i < _iw * _ih; i++) {
        uint8_t* px = &_pixels[i * 4];
        int g = _pixMap[i];
        if (g < 0) { px[0] = px[1] = px[2] = 23; px[3] = 255; continue; }
        float v = std::max(-1.0f, std::min(1.0f, S.board[g] * inv));
        float a = std::sqrt(std::fabs(v));
        px[0] = (uint8_t)(40 + (v > 0 ? 215 * a : 0));
        px[1] = (uint8_t)(40 + (v > 0 ? 130 * a : 110 * a));
        px[2] = (uint8_t)(48 + (v > 0 ? 0 : 207 * a));
        px[3] = 255;
    }
    CGColorSpaceRef cs = CGColorSpaceCreateDeviceRGB();
    CGContextRef bc = CGBitmapContextCreate(_pixels.data(), _iw, _ih, 8, _iw * 4, cs, (CGBitmapInfo)kCGImageAlphaNoneSkipLast);
    CGImageRef img = CGBitmapContextCreateImage(bc);
    NSRect br = [self boardRect];
    CGContextRef ctx = [NSGraphicsContext currentContext].CGContext;
    CGContextSaveGState(ctx);
    CGContextTranslateCTM(ctx, br.origin.x, br.origin.y + br.size.height);
    CGContextScaleCTM(ctx, 1, -1);
    CGContextSetInterpolationQuality(ctx, kCGInterpolationLow);
    CGContextDrawImage(ctx, CGRectMake(0, 0, br.size.width, br.size.height), img);
    CGContextRestoreGState(ctx);
    CGImageRelease(img); CGContextRelease(bc); CGColorSpaceRelease(cs);

    auto toView = [&](Vec2 p) { return NSMakePoint(br.origin.x + p.x / 1.40 * br.size.width, br.origin.y + (1 - p.y / 1.50) * br.size.height); };
    [[NSColor colorWithWhite:1 alpha:0.55] setStroke];
    for (const auto& line : E.design.board.bridges) {
        NSBezierPath* path = [NSBezierPath bezierPath];
        for (size_t i = 0; i < line.size(); i++) {
            if (i == 0) [path moveToPoint:toView(line[i])]; else [path lineToPoint:toView(line[i])];
        }
        path.lineWidth = 2;
        [path stroke];
    }
    // Where each ringing string meets the bridge.
    for (int si = 0; si < E.piano->stringCount() && si < (int)S.stringLevel.size(); si++) {
        float lv = S.stringLevel[si];
        if (lv < 2e-7f) continue;
        float a = std::min(1.0f, 0.25f + 0.12f * std::log10(lv / 2e-7f));
        [[NSColor colorWithRed:1 green:0.95 blue:0.6 alpha:a] setFill];
        NSPoint c = toView(E.piano->stringBridge(si));
        [[NSBezierPath bezierPathWithOvalInRect:NSMakeRect(c.x - 3, c.y - 3, 6, 6)] fill];
    }
    const Plate& P = E.piano->plate();
    [@"Soundboard displacement" drawAtPoint:NSMakePoint(20, 14) withAttributes:title];
    [[NSString stringWithFormat:@"%d nodes  %.0f x %.0f mm  full scale %.2f um", P.n, P.hx * 1e3, P.hy * 1e3, _boardScale * 1e6]
        drawAtPoint:NSMakePoint(20, 32) withAttributes:small];

    // Strings of the last played note.
    NSRect sr = NSMakeRect(470, 50, self.bounds.size.width - 490, 250);
    [[NSColor colorWithWhite:0.13 alpha:1] setFill];
    NSRectFill(sr);
    [[NSString stringWithFormat:@"Strings of MIDI note %d (agraffe left, bridge right)", S.focusMidi] drawAtPoint:NSMakePoint(470, 14) withAttributes:title];
    float smax = 1e-7f;
    for (int i = 0; i < 3; i++) for (int l = 0; l < S.shapeNodes[i]; l++) smax = std::max(smax, std::fabs(S.shape[i][l]));
    static float sScale = 1e-4f;
    sScale = std::max(smax, sScale * 0.98f);
    NSColor* cols[3] = {[NSColor colorWithRed:0.45 green:0.8 blue:1 alpha:1], [NSColor colorWithRed:1 green:0.75 blue:0.35 alpha:1],
                        [NSColor colorWithRed:0.6 green:1 blue:0.6 alpha:1]};
    for (int i = 0; i < 3; i++) {
        int N = S.shapeNodes[i];
        if (N < 2) continue;
        NSBezierPath* path = [NSBezierPath bezierPath];
        for (int l = 0; l < N; l++) {
            NSPoint q = NSMakePoint(sr.origin.x + 10 + (sr.size.width - 20) * l / (N - 1),
                                    sr.origin.y + sr.size.height / 2 - S.shape[i][l] / sScale * (sr.size.height * 0.42));
            if (l == 0) [path moveToPoint:q]; else [path lineToPoint:q];
        }
        [cols[i] setStroke];
        path.lineWidth = 1.4;
        [path stroke];
    }
    [[NSString stringWithFormat:@"full scale %.3f mm   felt force %.1f / %.1f / %.1f N", sScale * 1e3, S.hammerForce[0], S.hammerForce[1], S.hammerForce[2]]
        drawAtPoint:NSMakePoint(470, 32) withAttributes:small];

    // Status.
    float ld = E.load.load(), pk = E.loadPeak.load();
    CGFloat y = 396;
    auto line = [&](NSString* s, NSDictionary* at) { [s drawAtPoint:NSMakePoint(470, y) withAttributes:at]; y += 18; };
    int dp = E.demoPlaying.load();
    if (dp >= 0 && dp < (int)E.demos.size())
        line([NSString stringWithFormat:@"Playing: %s  (%d:%02d)", E.demos[dp].title.c_str(), (int)E.demoSeconds.load() / 60, (int)E.demoSeconds.load() % 60], title);
    else
        line(self.audioOn ? @"Audio on" : @"Audio off", title);
    line([NSString stringWithFormat:@"engine load %3.0f %%   peak %3.0f %%   overruns %d   helpers %s", ld * 100, pk * 100, E.overruns.load(),
                                    E.piano->helpersBusy() ? "sharing" : "idle"], small);
    line([NSString stringWithFormat:@"strings awake %d / %d    output peak %.2f    %.0f Hz", S.awake, E.piano->stringCount(), E.level.load(), E.sampleRate], small);
    y += 6;
    line(@"Play:  A W S E D F T G Y H U J K O L P ;    mouse on the keys    MIDI", small);
    line([NSString stringWithFormat:@"Z / X octave (now %d)    C / V velocity (now %d)    Space sustain    Return audio", self.octave, self.velocity], small);
    if (dp >= 0 && dp < (int)E.demos.size()) line([NSString stringWithFormat:@"%s, MAESTRO dataset (CC BY-NC-SA)", E.demos[dp].note.c_str()], small);

    // Keyboard.
    NSRect kr = [self keysRect];
    CGFloat ww = kr.size.width / 52.0;
    int w = 0;
    for (int m = 21; m <= 108; m++) {
        if (isBlack(m)) continue;
        NSRect k = NSMakeRect(kr.origin.x + w * ww, kr.origin.y, ww - 1, kr.size.height);
        [(_down[m] ? [NSColor colorWithRed:1 green:0.8 blue:0.4 alpha:1] : [NSColor colorWithWhite:0.93 alpha:1]) setFill];
        NSRectFill(k);
        w++;
    }
    w = 0;
    for (int m = 21; m <= 108; m++) {
        if (!isBlack(m)) { w++; continue; }
        NSRect k = NSMakeRect(kr.origin.x + w * ww - ww * 0.3, kr.origin.y, ww * 0.6, kr.size.height * 0.62);
        [(_down[m] ? [NSColor colorWithRed:0.9 green:0.6 blue:0.2 alpha:1] : [NSColor colorWithWhite:0.1 alpha:1]) setFill];
        NSRectFill(k);
    }
}

- (int)noteAt:(NSPoint)p {
    NSRect kr = [self keysRect];
    if (!NSPointInRect(p, kr)) return -1;
    CGFloat ww = kr.size.width / 52.0;
    int w = 0, hit = -1;
    for (int m = 21; m <= 108; m++) {
        if (isBlack(m)) {
            NSRect k = NSMakeRect(kr.origin.x + w * ww - ww * 0.3, kr.origin.y, ww * 0.6, kr.size.height * 0.62);
            if (NSPointInRect(p, k)) return m;
        } else {
            NSRect k = NSMakeRect(kr.origin.x + w * ww, kr.origin.y, ww, kr.size.height);
            if (NSPointInRect(p, k)) hit = m;
            w++;
        }
    }
    return hit;
}

- (void)press:(int)m velocity:(int)v { if (m < 21 || m > 108 || _down[m]) return; _down[m] = true; gEngine.uiEvents.push({1, m, v}); }
- (void)release:(int)m { if (m < 21 || m > 108 || !_down[m]) return; _down[m] = false; gEngine.uiEvents.push({0, m, 0}); }

- (void)mouseDown:(NSEvent*)e {
    NSPoint p = [self convertPoint:e.locationInWindow fromView:nil];
    int m = [self noteAt:p];
    if (m < 0) return;
    NSRect kr = [self keysRect];
    int v = (int)std::clamp(30 + 97 * (p.y - kr.origin.y) / kr.size.height, 20.0, 127.0);  // lower on the key = harder
    _mouseNote = m;
    [self press:m velocity:v];
}
- (void)mouseUp:(NSEvent*)e { if (_mouseNote >= 0) [self release:_mouseNote]; _mouseNote = -1; }

- (int)noteForKey:(unichar)c {
    static const char* keys = "awsedftgyhujkolp;";
    const char* f = strchr(keys, tolower(c));
    return f && c ? (self.octave + 1) * 12 + (int)(f - keys) : -1;
}
- (void)keyDown:(NSEvent*)e {
    if (e.isARepeat || e.characters.length == 0) return;
    unichar c = [e.charactersIgnoringModifiers characterAtIndex:0];
    int m = [self noteForKey:c];
    if (m >= 0) { [self press:m velocity:self.velocity]; return; }
    switch (tolower(c)) {
        case 'z': self.octave = std::max(0, self.octave - 1); break;
        case 'x': self.octave = std::min(7, self.octave + 1); break;
        case 'c': self.velocity = std::max(10, self.velocity - 15); break;
        case 'v': self.velocity = std::min(127, self.velocity + 15); break;
        case ' ': gEngine.uiEvents.push({2, 127, 0}); break;
        case '\r': [NSApp sendAction:@selector(toggleAudio:) to:nil from:self]; break;
        default: break;
    }
}
- (void)keyUp:(NSEvent*)e {
    if (e.characters.length == 0) return;
    unichar c = [e.charactersIgnoringModifiers characterAtIndex:0];
    if (c == ' ') { gEngine.uiEvents.push({2, 0, 0}); return; }
    // Release against every octave, in case it was shifted while a key was held.
    static const char* keys = "awsedftgyhujkolp;";
    const char* f = strchr(keys, tolower(c));
    if (f && c) for (int o = 0; o < 9; o++) [self release:o * 12 + (int)(f - keys)];
}
- (void)midiNote:(int)m on:(BOOL)on { if (m >= 0 && m < 128) _down[m] = on; }
@end

// ------------------------------------------------------------------------------ app

@interface AppDelegate : NSObject <NSApplicationDelegate>
- (PianoView*)setUp;
- (BOOL)openAudio;
- (void)toggleAudio:(id)sender;
- (void)showPanel:(id)sender;
- (void)buildPanel;
- (NSPanel*)panel;
@end

@implementation AppDelegate {
    NSWindow* _window;
    PianoView* _view;
    AudioUnit _unit;
    MIDIClientRef _midi;
    MIDIPortRef _port;
    NSTimer* _timer;
    NSButton* _audioButton;
    NSButton* _playButton;
    NSButton* _sidewaysBox;
    NSPopUpButton* _demoMenu;
    NSPopUpButton* _gridMenu;
    NSPopUpButton* _listenerMenu;
    NSPanel* _panel;
    NSMutableArray<NSSlider*>* _sliders;
    NSMutableArray<NSTextField*>* _values;
    NSSlider* _volume;
}

- (BOOL)openAudio {
    AudioComponentDescription d = {kAudioUnitType_Output, kAudioUnitSubType_DefaultOutput, kAudioUnitManufacturer_Apple, 0, 0};
    AudioComponent c = AudioComponentFindNext(NULL, &d);
    if (!c || AudioComponentInstanceNew(c, &_unit) != noErr) return NO;
    AudioStreamBasicDescription f = {};
    f.mSampleRate = gEngine.sampleRate;
    f.mFormatID = kAudioFormatLinearPCM;
    f.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagIsNonInterleaved;
    f.mChannelsPerFrame = 2; f.mBitsPerChannel = 32; f.mBytesPerFrame = 4; f.mBytesPerPacket = 4; f.mFramesPerPacket = 1;
    AudioUnitSetProperty(_unit, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, 0, &f, sizeof(f));
    AURenderCallbackStruct cb = {renderCallback, NULL};
    AudioUnitSetProperty(_unit, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, 0, &cb, sizeof(cb));
    if (AudioUnitInitialize(_unit) != noErr) return NO;
    os_workgroup_t wg = nullptr;
    UInt32 size = sizeof(wg);
    if (AudioUnitGetProperty(_unit, kAudioOutputUnitProperty_OSWorkgroup, kAudioUnitScope_Global, 0, &wg, &size) == noErr) gEngine.workgroup = wg;
    return YES;
}

- (void)toggleAudio:(id)sender {
    if (!_unit && ![self openAudio]) return;
    if (_view.audioOn) AudioOutputUnitStop(_unit); else { gEngine.loadPeak = 0; gEngine.overruns = 0; AudioOutputUnitStart(_unit); }
    _view.audioOn = !_view.audioOn;
    _audioButton.title = _view.audioOn ? @"Sound: on" : @"Sound: off";
    [_window makeFirstResponder:_view];
}

- (void)toggleDemo:(id)sender {
    if (gEngine.demoPlaying.load() >= 0) {
        gEngine.uiEvents.push({5, 0, 0});
    } else if (!gEngine.demos.empty()) {
        if (!_view.audioOn) [self toggleAudio:nil];
        gEngine.uiEvents.push({4, (int)_demoMenu.indexOfSelectedItem, 0});
    }
    [_view clearKeys];
    [_window makeFirstResponder:_view];
}

// Board grid or string model changed: build a new engine.
- (void)rebuild:(id)sender {
    BOOL was = _view.audioOn;
    if (was) [self toggleAudio:nil];
    const double scales[] = {1.0, 1.25, 1.5};
    gEngine.gridScale = scales[std::clamp((int)_gridMenu.indexOfSelectedItem, 0, 2)];
    gEngine.sideways = _sidewaysBox.state == NSControlStateValueOn;
    gEngine.listener = (int)_listenerMenu.indexOfSelectedItem;
    gEngine.build();
    [_view rebuildMap];
    [_view clearKeys];
    if (was) [self toggleAudio:nil];
    [_window makeFirstResponder:_view];
}

// One row of the Instrument panel: a physical quantity of the design.
struct Knob { const char* name; double GrandOptions::* field; double lo, hi; bool logScale; const char* format; };
static const Knob kKnobs[] = {
    {"Hammer felt hardness", &GrandOptions::hammerHardness, 0.25, 4.0, true, "x %.2f"},
    {"Hammer mass", &GrandOptions::hammerMass, 0.5, 2.0, true, "x %.2f"},
    {"Felt damping", &GrandOptions::feltDamping, 0.0, 4.0, false, "x %.2f"},
    {"Strike point along string", &GrandOptions::strikePosition, 0.6, 1.6, false, "x %.2f"},
    {"Unison detune", &GrandOptions::unisonDetuneCents, 0.0, 3.0, false, "%.2f cents"},
    {"String internal loss", &GrandOptions::stringLoss, 0.3, 4.0, true, "x %.2f"},
    {"Soundboard thickness", &GrandOptions::boardThickness, 0.6, 1.6, false, "x %.2f (8 mm)"},
    {"Soundboard loss", &GrandOptions::boardLoss, 0.3, 4.0, true, "x %.2f"},
    {"Rib height", &GrandOptions::ribHeight, 0.5, 1.6, false, "x %.2f (22 mm)"},
    {"Key knock on keybed", &GrandOptions::keyThump, 0.0, 20.0, false, "x %.1f"},
    {"Room size", &GrandOptions::roomSize, 0.5, 2.0, true, "x %.2f (9 x 7 x 4 m)"},
    {"Room absorption", &GrandOptions::roomAbsorption, 0.05, 0.6, false, "%.2f"},
    {"Lid angle", &GrandOptions::lidAngleDeg, 0.0, 60.0, false, "%.0f deg (0 = closed)"},
};
static const int kKnobCount = sizeof(kKnobs) / sizeof(kKnobs[0]);
static double knobToSlider(const Knob& k, double v) { return k.logScale ? std::log(v / k.lo) / std::log(k.hi / k.lo) : (v - k.lo) / (k.hi - k.lo); }
static double sliderToKnob(const Knob& k, double t) { return k.logScale ? k.lo * std::pow(k.hi / k.lo, t) : k.lo + t * (k.hi - k.lo); }

- (void)refreshKnobs {
    for (int i = 0; i < kKnobCount; i++) {
        double v = gEngine.voicing.*(kKnobs[i].field);
        _sliders[i].doubleValue = knobToSlider(kKnobs[i], v);
        _values[i].stringValue = [NSString stringWithFormat:@(kKnobs[i].format), v];
    }
}

// Sliders act when released: every change builds a new instrument.
- (void)knobChanged:(NSSlider*)sender {
    const Knob& k = kKnobs[sender.tag];
    gEngine.voicing.*(k.field) = sliderToKnob(k, sender.doubleValue);
    [self refreshKnobs];
    [self rebuild:nil];
}

- (void)resetKnobs:(id)sender {
    gEngine.voicing = GrandOptions();
    [self refreshKnobs];
    [self rebuild:nil];
}

- (void)showPanel:(id)sender {
    [self buildPanel];
    [_panel orderFront:nil];
}

- (void)buildPanel {
    if (!_panel) {
        CGFloat h = 60 + 30 * kKnobCount;
        _panel = [[NSPanel alloc] initWithContentRect:NSMakeRect(0, 0, 560, h)
                                            styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskUtilityWindow
                                              backing:NSBackingStoreBuffered defer:NO];
        _panel.title = @"Instrument";
        _panel.appearance = [NSAppearance appearanceNamed:NSAppearanceNameDarkAqua];
        _panel.floatingPanel = YES;
        _panel.releasedWhenClosed = NO;
        _sliders = [NSMutableArray new];
        _values = [NSMutableArray new];
        for (int i = 0; i < kKnobCount; i++) {
            CGFloat y = h - 36 - 30 * i;
            NSTextField* name = [NSTextField labelWithString:@(kKnobs[i].name)];
            name.frame = NSMakeRect(16, y, 170, 18);
            NSSlider* sl = [NSSlider sliderWithValue:0 minValue:0 maxValue:1 target:self action:@selector(knobChanged:)];
            sl.frame = NSMakeRect(190, y - 2, 200, 22);
            sl.tag = i;
            sl.continuous = NO;
            NSTextField* val = [NSTextField labelWithString:@""];
            val.frame = NSMakeRect(398, y, 156, 18);
            val.font = [NSFont monospacedDigitSystemFontOfSize:11 weight:NSFontWeightRegular];
            [_panel.contentView addSubview:name];
            [_panel.contentView addSubview:sl];
            [_panel.contentView addSubview:val];
            [_sliders addObject:sl];
            [_values addObject:val];
        }
        NSButton* reset = [NSButton buttonWithTitle:@"Reset to reference" target:self action:@selector(resetKnobs:)];
        reset.frame = NSMakeRect(12, 10, 160, 28);
        [_panel.contentView addSubview:reset];
        [self refreshKnobs];
        NSRect f = _window.frame;
        [_panel setFrameOrigin:NSMakePoint(NSMaxX(f) + 8, NSMaxY(f) - _panel.frame.size.height)];
    }
}

- (NSPanel*)panel { return _panel; }

- (void)volumeChanged:(id)sender { gEngine.volume = (float)_volume.doubleValue; [_window makeFirstResponder:_view]; }

- (void)buildControls {
    auto label = [&](NSString* text, CGFloat x, CGFloat y) {
        NSTextField* t = [NSTextField labelWithString:text];
        t.textColor = [NSColor colorWithWhite:0.75 alpha:1];
        t.font = [NSFont systemFontOfSize:11];
        t.frame = NSMakeRect(x, y + 4, 90, 16);
        [_view addSubview:t];
    };
    _audioButton = [NSButton buttonWithTitle:@"Sound: off" target:self action:@selector(toggleAudio:)];
    _audioButton.frame = NSMakeRect(466, 312, 110, 28);
    [_view addSubview:_audioButton];
    label(@"Volume", 590, 314);
    _volume = [NSSlider sliderWithValue:0.8 minValue:0.05 maxValue:3.0 target:self action:@selector(volumeChanged:)];
    _volume.frame = NSMakeRect(640, 314, 160, 24);
    [_view addSubview:_volume];

    _demoMenu = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(466, 346, 420, 26) pullsDown:NO];
    for (const auto& d : gEngine.demos) [_demoMenu addItemWithTitle:@(d.title.c_str())];
    if (gEngine.demos.empty()) [_demoMenu addItemWithTitle:@"No demo recordings found (run tools/fetch_demos.sh)"];
    [_view addSubview:_demoMenu];
    _playButton = [NSButton buttonWithTitle:@"Play / stop" target:self action:@selector(toggleDemo:)];
    _playButton.frame = NSMakeRect(892, 345, 110, 28);
    [_view addSubview:_playButton];
    NSButton* inst = [NSButton buttonWithTitle:@"Instrument..." target:self action:@selector(showPanel:)];
    inst.frame = NSMakeRect(1004, 345, 92, 28);
    [_view addSubview:inst];

    label(@"Board grid", 820, 314);
    _gridMenu = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(886, 312, 116, 26) pullsDown:NO];
    [_gridMenu addItemsWithTitles:@[@"finest", @"1.25x", @"1.5x"]];
    [_gridMenu selectItemAtIndex:2];
    _gridMenu.target = self; _gridMenu.action = @selector(rebuild:);
    [_view addSubview:_gridMenu];
    _listenerMenu = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(820, 370, 182, 26) pullsDown:NO];
    [_listenerMenu addItemsWithTitles:@[@"Listener: in the room", @"Listener: at the keys", @"No room, lid off"]];
    _listenerMenu.target = self; _listenerMenu.action = @selector(rebuild:);
    [_view addSubview:_listenerMenu];
    _sidewaysBox = [NSButton checkboxWithTitle:@"Sideways string motion (2x string cost)" target:self action:@selector(rebuild:)];
    _sidewaysBox.frame = NSMakeRect(468, 372, 340, 20);
    _sidewaysBox.contentTintColor = [NSColor whiteColor];
    [_view addSubview:_sidewaysBox];
}

- (void)openMIDI {
    __weak PianoView* view = _view;
    MIDIClientCreateWithBlock(CFSTR("Physical Piano"), &_midi, nil);
    MIDIInputPortCreateWithProtocol(_midi, CFSTR("in"), kMIDIProtocol_1_0, &_port, ^(const MIDIEventList* list, void*) {
        const MIDIEventPacket* pk = &list->packet[0];
        for (UInt32 i = 0; i < list->numPackets; i++, pk = MIDIEventPacketNext(pk))
            for (UInt32 w = 0; w < pk->wordCount; w++) {
                UInt32 word = pk->words[w];
                if ((word >> 28) != 2) continue;  // MIDI 1.0 channel voice
                int status = (word >> 20) & 0xF, d1 = (word >> 8) & 0x7F, d2 = word & 0x7F;
                if (status == 9 && d2 > 0) gEngine.midiEvents.push({1, d1, d2});
                else if (status == 8 || status == 9) gEngine.midiEvents.push({0, d1, 0});
                else if (status == 0xB && d1 == 64) gEngine.midiEvents.push({2, d2, 0});
                if (status == 8 || status == 9) {
                    BOOL on = status == 9 && d2 > 0;
                    dispatch_async(dispatch_get_main_queue(), ^{ [view midiNote:d1 on:on]; });
                }
            }
    });
    for (ItemCount i = 0; i < MIDIGetNumberOfSources(); i++) MIDIPortConnectSource(_port, MIDIGetSource(i), NULL);
}

- (PianoView*)setUp {
    gEngine.loadDemos(PP_DEMO_DIR);
    gEngine.build();
    _window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 1100, 640)
                                          styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskMiniaturizable
                                            backing:NSBackingStoreBuffered defer:NO];
    _window.title = @"Physical Piano";
    _view = [[PianoView alloc] initWithFrame:NSMakeRect(0, 0, 1100, 640)];
    _window.contentView = _view;
    _window.appearance = [NSAppearance appearanceNamed:NSAppearanceNameDarkAqua];
    [self buildControls];
    return _view;
}

- (void)applicationDidFinishLaunching:(NSNotification*)n {
    [self openAudio];  // opened, not started: no sound until asked
    [self setUp];
    [_window center];
    [_window makeKeyAndOrderFront:nil];
    [_window makeFirstResponder:_view];
    [self openMIDI];
    _timer = [NSTimer scheduledTimerWithTimeInterval:1.0 / 30 repeats:YES block:^(NSTimer*) { self->_view.needsDisplay = YES; }];
    [NSApp activateIgnoringOtherApps:YES];
}
- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication*)a { return YES; }
- (void)applicationWillTerminate:(NSNotification*)n { if (_unit) { AudioOutputUnitStop(_unit); AudioUnitUninitialize(_unit); } }
@end

// Headless check: drives the same render path the audio callback uses, with no device and
// no window, then writes a picture of the view.
static int selfTest(const char* png) {
    pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);  // stand in for the audio thread's priority
    [NSApplication sharedApplication];
    AppDelegate* d = [AppDelegate new];
    BOOL audio = [d openAudio];  // opened but never started, so helpers get the real thread group and nothing sounds
    PianoView* view = [d setUp];  // window is built but never shown
    // Rebuild the engine the way the grid and string controls do.
    for (double g : {1.0, 1.25, 1.5}) { gEngine.gridScale = g; gEngine.sideways = g == 1.25; gEngine.listener = g == 1.0 ? 1 : 2; gEngine.build(); [view rebuildMap]; }
    gEngine.listener = 0;
    gEngine.sideways = false;
    gEngine.build(); [view rebuildMap];
    printf("selftest: engine rebuilt 4 times, audio device %s, thread group %s\n", audio ? "opened" : "unavailable", gEngine.workgroup ? "joined" : "none");
    printf("selftest: %zu demo recordings loaded\n", gEngine.demos.size());
    std::vector<float> L(256), R(256);
    float peak = 0;
    for (int b = 0; b < 1500; b++) {
        if (b == 2) gEngine.uiEvents.push({4, 0, 0});
        gEngine.framesSinceSnap = 1 << 20;
        gEngine.render(L.data(), R.data(), 256);
        for (float x : L) peak = std::max(peak, std::fabs(x));
    }
    NSBitmapImageRep* rep = [view bitmapImageRepForCachingDisplayInRect:view.bounds];
    [view cacheDisplayInRect:view.bounds toBitmapImageRep:rep];
    [[rep representationUsingType:NSBitmapImageFileTypePNG properties:@{}] writeToFile:@(png) atomically:YES];
    {   // The Instrument panel, built but not shown on screen.
        gEngine.voicing.hammerHardness = 2.0;
        [d buildPanel];
        NSView* pv = [d panel].contentView;
        NSBitmapImageRep* pr = [pv bitmapImageRepForCachingDisplayInRect:pv.bounds];
        [pv cacheDisplayInRect:pv.bounds toBitmapImageRep:pr];
        [[pr representationUsingType:NSBitmapImageFileTypePNG properties:@{}] writeToFile:[@(png) stringByReplacingOccurrencesOfString:@".png" withString:@"_panel.png"] atomically:YES];
    }
    printf("selftest: engine load mean %.0f %%, worst block %.0f %%\n", gEngine.load.load() * 100, gEngine.loadPeak.load() * 100);
    printf("selftest: 8 s of the first demo rendered through the callback path, peak %.3f, awake %d, wrote %s\n", peak, gEngine.piano->awakeStrings(), png);
    return peak > 0.001f && std::isfinite(peak) ? 0 : 1;
}

// Measures the engine on the real audio thread, with the output forced to silence.
static int loadTest(int demo, double seconds, double grid, bool sideways) {
    [NSApplication sharedApplication];
    AppDelegate* d = [AppDelegate new];
    gEngine.mute = true;
    gEngine.gridScale = grid; gEngine.sideways = sideways;
    if (![d openAudio]) { printf("loadtest: no audio device\n"); return 1; }
    [d setUp];
    gEngine.gridScale = grid; gEngine.sideways = sideways;
    gEngine.build();
    [d toggleAudio:nil];
    gEngine.uiEvents.push({4, demo, 0});
    printf("loadtest: %s, board grid %.2fx, sideways %d, silent output\n", gEngine.demos[demo].title.c_str(), grid, (int)sideways);
    for (double t = 0; t < seconds; t += 5) {
        gEngine.loadPeak = 0;
        int o0 = gEngine.overruns.load(), b0 = gEngine.blocks.load();
        [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:5]];
        printf("  %3.0f-%3.0f s: mean load %3.0f %%, worst block %3.0f %%, blocks over budget %d of %d, strings awake %d\n", t, t + 5,
               gEngine.load.load() * 100, gEngine.loadPeak.load() * 100, gEngine.overruns.load() - o0, gEngine.blocks.load() - b0,
               gEngine.piano->awakeStrings());
    }
    [d toggleAudio:nil];
    return 0;
}

int main(int argc, const char** argv) {
    @autoreleasepool {
        if (argc > 2 && strcmp(argv[1], "--loadtest") == 0)
            return loadTest(atoi(argv[2]), argc > 3 ? atof(argv[3]) : 30, argc > 4 ? atof(argv[4]) : 1.5, argc > 5 && atoi(argv[5]));
        if (argc > 2 && strcmp(argv[1], "--selftest") == 0) return selfTest(argv[2]);
        NSApplication* app = [NSApplication sharedApplication];
        app.activationPolicy = NSApplicationActivationPolicyRegular;
        AppDelegate* d = [AppDelegate new];
        app.delegate = d;
        NSMenu* bar = [NSMenu new];
        NSMenuItem* item = [NSMenuItem new];
        [bar addItem:item];
        NSMenu* menu = [NSMenu new];
        [menu addItemWithTitle:@"Quit Physical Piano" action:@selector(terminate:) keyEquivalent:@"q"];
        item.submenu = menu;
        app.mainMenu = bar;
        [app run];
    }
    return 0;
}
