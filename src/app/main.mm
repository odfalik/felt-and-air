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
    GrandOptions voicing;  // what the Instrument window edits
    double lastPedal = 0;
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
        lastPedal = 0;
        piano->setSustain(false);
    }
    mach_timebase_info_data_t tb;

    void build() {
        GrandOptions g = voicing;
        g.twoPolarisations = sideways;
        design = makeGrand(g);
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
        piano->setPedal(lastPedal);  // a demo keeps playing across a rebuild; carry the pedal over
        l.assign(8192, 0); r.assign(8192, 0);
        mach_timebase_info(&tb);
    }
    void handle(const Event& e) {
        if (e.type == 1) { piano->noteOn(e.a, midiVelocityToHammerSpeed(e.b)); focus = e.a; }
        else if (e.type == 0) piano->noteOff(e.a);
        else if (e.type == 2) { lastPedal = e.a / 127.0; piano->setPedal(lastPedal); }
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

// ------------------------------------------------------------------------------ room plan

// Top-down drawing of the room. Drag the piano or the listener.
@interface RoomPlanView : NSView
@property(nonatomic, copy) void (^changed)(void);
@end

@implementation RoomPlanView {
    int _drag;  // 0 none, 1 listener, 2 piano
    NSPoint _grab;
}
- (BOOL)isFlipped { return NO; }
- (CGFloat)scale {
    const GrandOptions& o = gEngine.voicing;
    return std::min((self.bounds.size.width - 20) / o.roomLx, (self.bounds.size.height - 44) / o.roomLy);
}
- (NSPoint)toView:(double)x :(double)y {
    const GrandOptions& o = gEngine.voicing;
    CGFloat s = [self scale];
    return NSMakePoint(self.bounds.size.width / 2 + (x - o.roomLx / 2) * s, self.bounds.size.height / 2 + 10 + (y - o.roomLy / 2) * s);
}
- (void)drawRect:(NSRect)dirty {
    const GrandOptions& o = gEngine.voicing;
    [[NSColor colorWithWhite:0.12 alpha:1] setFill];
    NSRectFill(self.bounds);
    NSPoint a = [self toView:0 :0], b = [self toView:o.roomLx :o.roomLy];
    NSRect room = NSMakeRect(a.x, a.y, b.x - a.x, b.y - a.y);
    [(o.roomEnabled ? [NSColor colorWithWhite:0.2 alpha:1] : [NSColor colorWithWhite:0.14 alpha:1]) setFill];
    NSRectFill(room);
    [[NSColor colorWithWhite:o.roomEnabled ? 0.75 : 0.35 alpha:1] setStroke];
    NSBezierPath* walls = [NSBezierPath bezierPathWithRect:room];
    walls.lineWidth = o.roomEnabled ? 2 : 1;
    if (!o.roomEnabled) { CGFloat dash[] = {4, 4}; [walls setLineDash:dash count:2 phase:0]; }
    [walls stroke];
    // Piano: the soundboard outline, with the keyboard along its near edge.
    NSBezierPath* piano = [NSBezierPath bezierPath];
    const auto& outline = gEngine.design.board.outline;
    for (size_t i = 0; i < outline.size(); i++) {
        NSPoint q = [self toView:o.pianoX + outline[i].x :o.pianoY + outline[i].y];
        if (i == 0) [piano moveToPoint:q]; else [piano lineToPoint:q];
    }
    [piano closePath];
    [[NSColor colorWithRed:0.75 green:0.55 blue:0.3 alpha:1] setFill];
    [piano fill];
    NSPoint k0 = [self toView:o.pianoX :o.pianoY - 0.16], k1 = [self toView:o.pianoX + 1.40 :o.pianoY];
    [[NSColor colorWithWhite:0.92 alpha:1] setFill];
    NSRectFill(NSMakeRect(k0.x, k0.y, k1.x - k0.x, k1.y - k0.y));
    if (o.lidAngleDeg > 1) {  // the lid is hinged on the bass side and opens toward the treble
        [[NSColor colorWithWhite:1 alpha:0.8] setStroke];
        NSBezierPath* hinge = [NSBezierPath bezierPath];
        [hinge moveToPoint:[self toView:o.pianoX :o.pianoY]];
        [hinge lineToPoint:[self toView:o.pianoX :o.pianoY + 1.5]];
        hinge.lineWidth = 3;
        [hinge stroke];
    }
    // Listener: two ears and the direction faced.
    [[NSColor colorWithRed:0.45 green:0.8 blue:1 alpha:1] setFill];
    for (int e = 0; e < 2; e++) {
        NSPoint q = [self toView:o.pianoX + gEngine.design.ears[e].x :o.pianoY + gEngine.design.ears[e].y];
        [[NSBezierPath bezierPathWithOvalInRect:NSMakeRect(q.x - 4, q.y - 4, 8, 8)] fill];
    }
    NSPoint h = [self toView:o.listenerX :o.listenerY], c = [self toView:o.pianoX + 0.7 :o.pianoY + 0.6];
    CGFloat len = hypot(c.x - h.x, c.y - h.y);
    if (len > 1) {
        [[NSColor colorWithRed:0.45 green:0.8 blue:1 alpha:0.7] setStroke];
        NSBezierPath* look = [NSBezierPath bezierPath];
        [look moveToPoint:h];
        [look lineToPoint:NSMakePoint(h.x + (c.x - h.x) / len * 14, h.y + (c.y - h.y) / len * 14)];
        look.lineWidth = 2;
        [look stroke];
    }
    NSDictionary* small = @{NSFontAttributeName: [NSFont systemFontOfSize:10.5], NSForegroundColorAttributeName: [NSColor colorWithWhite:0.7 alpha:1]};
    double dist = hypot(o.listenerX - (o.pianoX + 0.7), o.listenerY - (o.pianoY + 0.6));
    [[NSString stringWithFormat:@"Drag the piano or the listener.   %.1f x %.1f m   listener %.1f m from the board", o.roomLx, o.roomLy, dist]
        drawAtPoint:NSMakePoint(6, 4) withAttributes:small];
}
- (void)mouseDown:(NSEvent*)e {
    const GrandOptions& o = gEngine.voicing;
    NSPoint p = [self convertPoint:e.locationInWindow fromView:nil];
    NSPoint h = [self toView:o.listenerX :o.listenerY];
    NSPoint pa = [self toView:o.pianoX :o.pianoY - 0.2], pb = [self toView:o.pianoX + 1.4 :o.pianoY + 1.5];
    _drag = 0;
    if (hypot(p.x - h.x, p.y - h.y) < 16) { _drag = 1; _grab = NSMakePoint(p.x - h.x, p.y - h.y); }
    else if (p.x >= pa.x && p.x <= pb.x && p.y >= pa.y && p.y <= pb.y) { _drag = 2; NSPoint q = [self toView:o.pianoX :o.pianoY]; _grab = NSMakePoint(p.x - q.x, p.y - q.y); }
}
- (void)mouseDragged:(NSEvent*)e {
    if (!_drag) return;
    GrandOptions& o = gEngine.voicing;
    NSPoint p = [self convertPoint:e.locationInWindow fromView:nil];
    CGFloat s = [self scale];
    double x = o.roomLx / 2 + (p.x - _grab.x - self.bounds.size.width / 2) / s, y = o.roomLy / 2 + (p.y - _grab.y - self.bounds.size.height / 2 - 10) / s;
    if (_drag == 1) { o.listenerX = std::clamp(x, 0.3, o.roomLx - 0.3); o.listenerY = std::clamp(y, 0.3, o.roomLy - 0.3); }
    else { o.pianoX = std::clamp(x, 0.2, std::max(0.2, o.roomLx - 1.6)); o.pianoY = std::clamp(y, 0.8, std::max(0.8, o.roomLy - 1.7)); }
    gEngine.design = makeGrand(o);  // only to redraw the ears while dragging; the engine is rebuilt on release
    self.needsDisplay = YES;
}
- (void)mouseUp:(NSEvent*)e {
    if (_drag && self.changed) self.changed();
    _drag = 0;
}
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
    NSPanel* _panel;
    NSMutableArray<NSSlider*>* _sliders;
    NSMutableArray<NSTextField*>* _values;
    NSPopUpButton* _presetMenu;
    NSButton* _roomBox;
    NSButton* _stretchBox;
    RoomPlanView* _plan;
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
    gEngine.build();
    [_view rebuildMap];
    [_view clearKeys];
    if (was) [self toggleAudio:nil];
    [_window makeFirstResponder:_view];
}

// ---- Instrument window ---------------------------------------------------------------
// One adjustable quantity of the design. Every one is a physical number.
struct Knob { int tab; const char* name; double GrandOptions::* field; double lo, hi; bool logScale; const char* format; const char* help; };
enum { kSpace, kHammers, kStrings, kBoard, kTabCount };
static const char* kTabNames[] = {"Space", "Hammers", "Strings", "Soundboard"};
static const Knob kKnobs[] = {
    {kSpace, "Room length", &GrandOptions::roomLx, 3.0, 30.0, true, "%.1f m", "Along the keyboard direction."},
    {kSpace, "Room width", &GrandOptions::roomLy, 3.0, 30.0, true, "%.1f m", "Along the strings."},
    {kSpace, "Room height", &GrandOptions::roomLz, 2.2, 15.0, true, "%.1f m", ""},
    {kSpace, "Wall absorption", &GrandOptions::roomAbsorption, 0.03, 0.7, true, "%.2f", "Share of sound each surface soaks up. Low is a tiled hall, high is a carpeted studio."},
    {kSpace, "Ear height", &GrandOptions::earHeightM, 0.6, 2.5, false, "%.2f m", "Above the floor. The soundboard sits at 0.75 m."},
    {kSpace, "Ear spacing", &GrandOptions::earSpacingM, 0.1, 1.5, false, "%.2f m", "A spaced pair; wider gives a wider image."},
    {kSpace, "Lid angle", &GrandOptions::lidAngleDeg, 0.0, 60.0, false, "%.0f deg", "0 takes the lid's reflection away."},
    {kHammers, "Felt hardness", &GrandOptions::hammerHardness, 0.2, 5.0, true, "x %.2f", "Felt stiffness. Harder is brighter."},
    {kHammers, "Hammer mass", &GrandOptions::hammerMass, 0.5, 2.0, true, "x %.2f", "Heavier stays on the string longer: louder and duller."},
    {kHammers, "Felt damping", &GrandOptions::feltDamping, 0.0, 5.0, false, "x %.2f", "Energy the felt loses as it is squeezed."},
    {kHammers, "Strike point", &GrandOptions::strikePosition, 0.5, 1.8, false, "x %.2f", "Distance from the agraffe; the reference is about 1/8 of the string."},
    {kHammers, "Key knock", &GrandOptions::keyThump, 0.0, 30.0, false, "x %.1f", "Knock of the key landing on the keybed."},
    {kHammers, "Damper strength", &GrandOptions::damperStrength, 0.2, 4.0, true, "x %.2f", "How fast a released note is stopped."},
    {kStrings, "Pitch of A4", &GrandOptions::a4Hz, 415.0, 466.0, false, "%.1f Hz", ""},
    {kStrings, "Unison detune", &GrandOptions::unisonDetuneCents, 0.0, 6.0, false, "%.2f cents", "Spread between the strings of one note. More gives beating, then honky-tonk."},
    {kStrings, "Internal loss", &GrandOptions::stringLoss, 0.25, 5.0, true, "x %.2f", "The string's own damping, apart from what the board takes."},
    {kStrings, "Wire stiffness", &GrandOptions::stringStiffness, 0.25, 4.0, true, "x %.2f", "Bending stiffness. More makes the overtones sharper."},
    {kStrings, "Bridge axis tilt", &GrandOptions::bridgeTiltDeg, 0.0, 30.0, false, "%.0f deg", "Couples sideways string motion to the board (needs sideways motion on)."},
    {kBoard, "Thickness", &GrandOptions::boardThicknessMm, 5.0, 13.0, false, "%.1f mm", ""},
    {kBoard, "Loss factor", &GrandOptions::boardLossPercent, 0.5, 8.0, true, "%.1f %%", "Internal damping of the wood."},
    {kBoard, "Rib height", &GrandOptions::ribHeightMm, 10.0, 36.0, false, "%.0f mm", ""},
    {kBoard, "Rib spacing", &GrandOptions::ribSpacingMm, 80.0, 220.0, false, "%.0f mm", ""},
    {kBoard, "Crown radius", &GrandOptions::crownRadiusM, 6.0, 80.0, true, "%.0f m", "Smaller is a more domed, stiffer board."},
    {kBoard, "Rim grip", &GrandOptions::rimClamp, 0.0, 1.0, false, "%.2f", "0: edge free to tilt. 1: edge clamped."},
    {kBoard, "Bridge height", &GrandOptions::bridgeHeightMm, 20.0, 50.0, false, "%.0f mm", ""},
};
static const int kKnobCount = sizeof(kKnobs) / sizeof(kKnobs[0]);
// Fields that have no slider but belong to a saved setup.
static double GrandOptions::* const kPlanFields[] = {&GrandOptions::pianoX, &GrandOptions::pianoY, &GrandOptions::listenerX, &GrandOptions::listenerY};
static double knobToSlider(const Knob& k, double v) { return k.logScale ? std::log(v / k.lo) / std::log(k.hi / k.lo) : (v - k.lo) / (k.hi - k.lo); }
static double sliderToKnob(const Knob& k, double t) { return k.logScale ? k.lo * std::pow(k.hi / k.lo, t) : k.lo + t * (k.hi - k.lo); }

struct Preset { const char* name; void (*apply)(GrandOptions&); };
static const Preset kPresets[] = {
    {"Recital room, listener by the lid", [](GrandOptions& o) { o = GrandOptions(); }},
    {"At the keyboard", [](GrandOptions& o) { GrandOptions d; o.roomEnabled = true; o.roomLx = d.roomLx; o.roomLy = d.roomLy; o.roomLz = d.roomLz; o.roomAbsorption = d.roomAbsorption;
                                              o.pianoX = d.pianoX; o.pianoY = d.pianoY; o.listenerX = d.pianoX + 0.70; o.listenerY = d.pianoY - 0.45; o.earHeightM = 1.2; o.earSpacingM = 0.18; }},
    {"Small studio, close", [](GrandOptions& o) { o.roomEnabled = true; o.roomLx = 5.0; o.roomLy = 4.2; o.roomLz = 2.7; o.roomAbsorption = 0.40; o.pianoX = 1.2; o.pianoY = 1.3;
                                                   o.listenerX = 3.6; o.listenerY = 1.9; o.earHeightM = 1.3; o.earSpacingM = 0.5; }},
    {"Concert hall, tenth row", [](GrandOptions& o) { o.roomEnabled = true; o.roomLx = 28; o.roomLy = 18; o.roomLz = 12; o.roomAbsorption = 0.16; o.pianoX = 5.0; o.pianoY = 7.0;
                                                       o.listenerX = 15.0; o.listenerY = 7.8; o.earHeightM = 1.2; o.earSpacingM = 0.3; }},
    {"Stone chapel", [](GrandOptions& o) { o.roomEnabled = true; o.roomLx = 16; o.roomLy = 8; o.roomLz = 9; o.roomAbsorption = 0.06; o.pianoX = 4.0; o.pianoY = 2.5;
                                            o.listenerX = 9.0; o.listenerY = 3.2; o.earHeightM = 1.2; o.earSpacingM = 0.4; }},
    {"No room (open air), mics over the strings", [](GrandOptions& o) { o.roomEnabled = false; o.lidAngleDeg = 0; o.listenerX = o.pianoX + 0.70; o.listenerY = o.pianoY + 0.50; o.earHeightM = 1.65; o.earSpacingM = 0.7; }},
    {"Voicing: bright", [](GrandOptions& o) { o.hammerHardness = 2.2; o.feltDamping = 0.6; o.hammerMass = 0.9; }},
    {"Voicing: mellow", [](GrandOptions& o) { o.hammerHardness = 0.45; o.feltDamping = 1.6; o.hammerMass = 1.1; }},
    {"Voicing: reference", [](GrandOptions& o) { GrandOptions d; o.hammerHardness = d.hammerHardness; o.feltDamping = d.feltDamping; o.hammerMass = d.hammerMass; o.strikePosition = d.strikePosition; }},
    {"Honky-tonk tuning", [](GrandOptions& o) { o.unisonDetuneCents = 5.0; }},
};
static const int kPresetCount = sizeof(kPresets) / sizeof(kPresets[0]);

- (void)refreshKnobs {
    for (int i = 0; i < kKnobCount; i++) {
        double v = gEngine.voicing.*(kKnobs[i].field);
        _sliders[i].doubleValue = knobToSlider(kKnobs[i], v);
        _values[i].stringValue = [NSString stringWithFormat:@(kKnobs[i].format), v];
    }
    _roomBox.state = gEngine.voicing.roomEnabled ? NSControlStateValueOn : NSControlStateValueOff;
    _stretchBox.state = gEngine.voicing.stretchTuning ? NSControlStateValueOn : NSControlStateValueOff;
    _plan.needsDisplay = YES;
}

// Keep the piano and the listener inside the room after any change.
- (void)fitToRoom {
    GrandOptions& o = gEngine.voicing;
    o.pianoX = std::clamp(o.pianoX, 0.2, std::max(0.2, o.roomLx - 1.6));
    o.pianoY = std::clamp(o.pianoY, 0.8, std::max(0.8, o.roomLy - 1.7));
    o.listenerX = std::clamp(o.listenerX, 0.3, o.roomLx - 0.3);
    o.listenerY = std::clamp(o.listenerY, 0.3, o.roomLy - 0.3);
    o.earHeightM = std::min(o.earHeightM, o.roomLz - 0.2);
}

- (void)applyVoicing {
    [self fitToRoom];
    [self refreshKnobs];
    [self rebuild:nil];
}

// Sliders act when released: every change builds a new instrument.
- (void)knobChanged:(NSSlider*)sender {
    const Knob& k = kKnobs[sender.tag];
    gEngine.voicing.*(k.field) = sliderToKnob(k, sender.doubleValue);
    [self applyVoicing];
}
- (void)boxChanged:(id)sender {
    gEngine.voicing.roomEnabled = _roomBox.state == NSControlStateValueOn;
    gEngine.voicing.stretchTuning = _stretchBox.state == NSControlStateValueOn;
    [self applyVoicing];
}
- (void)planChanged { [self applyVoicing]; }

- (NSDictionary*)savedSetups { return [[NSUserDefaults standardUserDefaults] dictionaryForKey:@"setups"] ?: @{}; }

- (void)fillPresetMenu {
    [_presetMenu removeAllItems];
    [_presetMenu addItemWithTitle:@"Presets"];
    for (int i = 0; i < kPresetCount; i++) [_presetMenu addItemWithTitle:@(kPresets[i].name)];
    NSArray* saved = [[[self savedSetups] allKeys] sortedArrayUsingSelector:@selector(compare:)];
    if (saved.count) [_presetMenu.menu addItem:[NSMenuItem separatorItem]];
    for (NSString* name in saved) [_presetMenu addItemWithTitle:[@"Saved: " stringByAppendingString:name]];
    [_presetMenu.menu addItem:[NSMenuItem separatorItem]];
    [_presetMenu addItemWithTitle:@"Save current setup..."];
    [_presetMenu addItemWithTitle:@"Reset everything"];
}

- (void)presetChosen:(id)sender {
    NSString* title = _presetMenu.titleOfSelectedItem;
    NSInteger idx = _presetMenu.indexOfSelectedItem;
    GrandOptions& o = gEngine.voicing;
    if (idx >= 1 && idx <= kPresetCount) {
        kPresets[idx - 1].apply(o);
    } else if ([title hasPrefix:@"Saved: "]) {
        NSDictionary* d = [self savedSetups][[title substringFromIndex:7]];
        for (int i = 0; i < kKnobCount; i++) if (d[@(kKnobs[i].name)]) o.*(kKnobs[i].field) = [d[@(kKnobs[i].name)] doubleValue];
        const char* planNames[] = {"pianoX", "pianoY", "listenerX", "listenerY"};
        for (int i = 0; i < 4; i++) if (d[@(planNames[i])]) o.*(kPlanFields[i]) = [d[@(planNames[i])] doubleValue];
        if (d[@"room"]) o.roomEnabled = [d[@"room"] boolValue];
        if (d[@"stretch"]) o.stretchTuning = [d[@"stretch"] boolValue];
    } else if ([title isEqualToString:@"Reset everything"]) {
        o = GrandOptions();
    } else if ([title hasPrefix:@"Save current"]) {
        NSAlert* alert = [NSAlert new];
        alert.messageText = @"Save this setup as";
        NSTextField* field = [[NSTextField alloc] initWithFrame:NSMakeRect(0, 0, 240, 24)];
        alert.accessoryView = field;
        [alert addButtonWithTitle:@"Save"];
        [alert addButtonWithTitle:@"Cancel"];
        if ([alert runModal] == NSAlertFirstButtonReturn && field.stringValue.length) {
            NSMutableDictionary* d = [NSMutableDictionary new];
            for (int i = 0; i < kKnobCount; i++) d[@(kKnobs[i].name)] = @(o.*(kKnobs[i].field));
            const char* planNames[] = {"pianoX", "pianoY", "listenerX", "listenerY"};
            for (int i = 0; i < 4; i++) d[@(planNames[i])] = @(o.*(kPlanFields[i]));
            d[@"room"] = @(o.roomEnabled); d[@"stretch"] = @(o.stretchTuning);
            NSMutableDictionary* all = [[self savedSetups] mutableCopy];
            all[field.stringValue] = d;
            [[NSUserDefaults standardUserDefaults] setObject:all forKey:@"setups"];
        }
        [self fillPresetMenu];
        return;
    }
    [_presetMenu selectItemAtIndex:0];
    [self applyVoicing];
}

- (void)showPanel:(id)sender {
    [self buildPanel];
    [_panel orderFront:nil];
}

- (void)buildPanel {
    if (_panel) return;
    const CGFloat W = 600, H = 620;
    _panel = [[NSPanel alloc] initWithContentRect:NSMakeRect(0, 0, W, H)
                                        styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskUtilityWindow
                                          backing:NSBackingStoreBuffered defer:NO];
    _panel.title = @"Instrument";
    _panel.appearance = [NSAppearance appearanceNamed:NSAppearanceNameDarkAqua];
    _panel.floatingPanel = YES;
    _panel.releasedWhenClosed = NO;
    _sliders = [NSMutableArray new];
    _values = [NSMutableArray new];

    _presetMenu = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(14, H - 40, 330, 26) pullsDown:NO];
    _presetMenu.target = self; _presetMenu.action = @selector(presetChosen:);
    [self fillPresetMenu];
    [_panel.contentView addSubview:_presetMenu];
    NSTextField* hint = [NSTextField labelWithString:@"Changes apply when you let go."];
    hint.textColor = [NSColor secondaryLabelColor];
    hint.font = [NSFont systemFontOfSize:11];
    hint.frame = NSMakeRect(356, H - 36, 230, 16);
    [_panel.contentView addSubview:hint];

    NSTabView* tabs = [[NSTabView alloc] initWithFrame:NSMakeRect(6, 6, W - 12, H - 52)];
    [_panel.contentView addSubview:tabs];
    NSView* pages[kTabCount];
    CGFloat rowY[kTabCount];
    for (int t = 0; t < kTabCount; t++) {
        NSTabViewItem* item = [[NSTabViewItem alloc] initWithIdentifier:@(t)];
        item.label = @(kTabNames[t]);
        pages[t] = [[NSView alloc] initWithFrame:NSMakeRect(0, 0, W - 40, H - 100)];
        item.view = pages[t];
        [tabs addTabViewItem:item];
        rowY[t] = H - 140;
    }
    // Space tab: the plan on top, then the room switch.
    _plan = [[RoomPlanView alloc] initWithFrame:NSMakeRect(10, H - 100 - 262, W - 60, 254)];
    __weak AppDelegate* me = self;
    _plan.changed = ^{ [me planChanged]; };
    [pages[kSpace] addSubview:_plan];
    _roomBox = [NSButton checkboxWithTitle:@"Room (walls, floor, ceiling, reverberation)" target:self action:@selector(boxChanged:)];
    _roomBox.frame = NSMakeRect(12, H - 100 - 290, 400, 20);
    [pages[kSpace] addSubview:_roomBox];
    rowY[kSpace] = H - 100 - 322;
    _stretchBox = [NSButton checkboxWithTitle:@"Stretch tuning (octaves tuned by matching overtones)" target:self action:@selector(boxChanged:)];
    _stretchBox.frame = NSMakeRect(12, rowY[kStrings], 420, 20);
    [pages[kStrings] addSubview:_stretchBox];
    rowY[kStrings] -= 34;

    for (int i = 0; i < kKnobCount; i++) {
        const Knob& k = kKnobs[i];
        CGFloat y = rowY[k.tab];
        rowY[k.tab] -= k.tab == kSpace ? 27 : 44;
        NSTextField* name = [NSTextField labelWithString:@(k.name)];
        name.frame = NSMakeRect(12, y, 130, 18);
        NSSlider* sl = [NSSlider sliderWithValue:0 minValue:0 maxValue:1 target:self action:@selector(knobChanged:)];
        sl.frame = NSMakeRect(146, y - 2, 270, 22);
        sl.tag = i;
        sl.continuous = NO;
        sl.toolTip = @(k.help);
        NSTextField* val = [NSTextField labelWithString:@""];
        val.frame = NSMakeRect(426, y, 120, 18);
        val.font = [NSFont monospacedDigitSystemFontOfSize:12 weight:NSFontWeightRegular];
        [pages[k.tab] addSubview:name];
        [pages[k.tab] addSubview:sl];
        [pages[k.tab] addSubview:val];
        if (k.tab != kSpace && strlen(k.help)) {
            NSTextField* help = [NSTextField labelWithString:@(k.help)];
            help.textColor = [NSColor secondaryLabelColor];
            help.font = [NSFont systemFontOfSize:10.5];
            help.frame = NSMakeRect(148, y - 18, 400, 14);
            [pages[k.tab] addSubview:help];
        }
        [_sliders addObject:sl];
        [_values addObject:val];
    }
    [self refreshKnobs];
    NSRect f = _window.frame;
    [_panel setFrameOrigin:NSMakePoint(NSMaxX(f) + 8, NSMaxY(f) - _panel.frame.size.height)];
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
    for (double g : {1.0, 1.25, 1.5}) { gEngine.gridScale = g; gEngine.sideways = g == 1.25; gEngine.build(); [view rebuildMap]; }
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
    {   // The Instrument window, built but not shown on screen: one picture per tab.
        [d buildPanel];
        NSView* pv = [d panel].contentView;
        NSTabView* tabs = nil;
        for (NSView* v in pv.subviews) if ([v isKindOfClass:[NSTabView class]]) tabs = (NSTabView*)v;
        for (int t = 0; t < kTabCount; t++) {
            [tabs selectTabViewItemAtIndex:t];
            [pv layoutSubtreeIfNeeded];
            NSBitmapImageRep* outRep = [pv bitmapImageRepForCachingDisplayInRect:pv.bounds];
            NSGraphicsContext* gc = [NSGraphicsContext graphicsContextWithBitmapImageRep:outRep];
            [NSGraphicsContext saveGraphicsState];
            [NSGraphicsContext setCurrentContext:gc];
            [[NSColor colorWithWhite:0.16 alpha:1] setFill];
            NSRectFill(pv.bounds);
            [NSGraphicsContext restoreGraphicsState];
            [pv cacheDisplayInRect:pv.bounds toBitmapImageRep:outRep];
            NSString* path = [@(png) stringByReplacingOccurrencesOfString:@".png" withString:[NSString stringWithFormat:@"_panel%d.png", t]];
            [[outRep representationUsingType:NSBitmapImageFileTypePNG properties:@{}] writeToFile:path atomically:YES];
        }
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
