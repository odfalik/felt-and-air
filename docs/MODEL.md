# Felt & Air: model, validation, and limits

The engine uses double precision. Representative numerical results below are from
`make test` on a MacBook Pro M4 Max at 48 kHz (2026-11-18), with higher rates where stated.
These checks establish numerical properties, not agreement with measurements of a real
piano. Performance measurements are identified separately; offline deadlines are not
proof of dropout-free operation on a real audio device.

## What is simulated

One discrete mechanical system, `M q'' + C q' + K q = f`, stepped in time. `K` is symmetric
and comes from a discrete strain energy, so every part exchanges energy with every other
part consistently.

| Part | Model |
|---|---|
| Strings | Stiff string (tension + bending), 228 strings for 88 keys, 5 to 200 grid nodes each |
| Sideways motion | The second polarisation of every struck string. Its end follows the bridge top as the board tilts under it (the board's slope across the string times the bridge height), so the coupling has no free parameter. Only the angle at which the hammer strikes (4 degrees off upright) is assumed. Lengthens the tail of a note by up to 9 dB at 12 s |
| Stretching | Longitudinal motion on its own coarse grid, driven by the transverse wave; its tension change presses on the bridge through the downbearing angle. One-way: it does not act back on the transverse motion |
| Duplex | 99 free lengths of the treble wires behind the bridge, a third or half as long, never struck or damped |
| Tuning | A4 = 440 Hz, octaves tuned outward by matching partials (4:2, 2:1 in the high treble, 6:3 in the bass). Gives -14 cents at A0 and +34 cents at C8 |
| String loss | Constant plus frequency-squared decay rate. The constant is 0.12 /s, not the 0.5 /s of Chaigne and Askenfelt (1994), which was fitted with a rigid bridge and so contained the soundboard's share |
| Hammer | Head on a flexible shank (two masses and a spring), nonlinear felt `F = K z^p` with Hunt-Crossley damping, one hammer striking all strings of its note |
| Action after let-off | The hammer is released 2 mm under the string and flies there under gravity (7.6 ms at pianissimo, 1.0 ms at mezzo forte, 0.4 ms at fortissimo), comes back and is caught on the backcheck felt, then falls to rest when the key is released. The key lands on its rail felts going down and coming up. Felts are stiffening springs with drag; a share of their changing force reaches the board. The **Action knocks** control scales keybed landing, key return rail and hammer backcheck/rest knocks together |
| Pedals | Sustain is continuous. Soft (una corda) makes each hammer miss one string of three, on softer felt. Sostenuto holds up the dampers of the keys down when it is pressed. The sustain pedal work knocks at each end of its travel and rustles faintly while moving. **Pedal noise** scales only these endpoint knocks and moving friction, not key-return knocks |
| Dampers | Felt that rests on the string with a small force and soaks up its motion. Lifting it releases the slightly deflected string, which is the stir of the strings when the pedal goes down (26 dB below a mezzo forte note). A settled damped string is set aside and wakes in its bent shape; optionally damped strings stay in the simulation and answer faintly to the rest. The pedal is continuous: dampers graze the strings between 34 % and 47 % of its travel |
| Soundboard | Orthotropic Kirchhoff plate on a grain-aligned grid, grand-shaped outline, clamped rim, crown as a shallow-shell stiffness |
| Air on the board | Each quarter-metre patch feels its own motion as a piston (mass-like at low, resistive at high frequency) and the delayed acceleration of every other patch, on both faces. Lowers the lowest resonance by 8 % |
| Ribs | Beams across the grain, with a composite-section stiffness and tapered ends |
| Bridges | Beams along the bass and treble bridge lines, tied to the plate by interpolation |
| String to bridge | The string's end node is the plate at that point. Force and motion pass both ways with no extra parameters |
| Radiation | Each side of the board radiates into its own half space with opposite sign, summed node by node to two ears |
| Room | Floor, open lid, walls and ceiling as mirror images up to second order over quarter-metre board patches; a late field from a feedback delay network scaled to Sabine's decay time and level |

## Touch and timing

A MIDI note-on starts the hammer at let-off, not at string contact. MIDI16–127 retain
`0.25 + 5.5 * (velocity / 127)^1.7` m/s exactly. On this experimental branch, MIDI1–15
use a cubic launch-speed segment from 0.200m/s at MIDI1, joining the original value and
slope at16. This changes physical hammer flight/contact, not post-synthesis gain. The
0.200m/s lower endpoint is a hypothesis near one tested grazing regime, not a universal
piano/keyboard calibration or a guarantee of contact/audibility. Velocity zero remains
NoteOff in MIDI decoding, never a quiet strike. The configured release gap
(default 2 mm), gravity and moving string determine the subsequent contact time: roughly
7.6 ms at 0.3 m/s, 1 ms at 2 m/s and 0.4 ms at 5 m/s in the test case.

The same velocity uses the same launch speed and gap across the keyboard; there is no
extra bass-note delay. Grazing launches can take longer to reach a string or miss it,
especially when the gap/gravity/preload differ; no minimum hold or timing compensation
is inserted. The keyboard's actual velocity distribution is unknown, and a controller
that never emits MIDI1–15 will not exercise this candidate. Different hammer masses affect contact, but mass alone does not
slow free flight at a given initial speed. The current masses are assumed per-string values
multiplied by unison count, not a measured monotonically decreasing hammer-weight scale.
The preceding finger/key/linkage motion is not part of live synthesis. Adding its whole
stroke duration after a MIDI event would duplicate latency already incurred by the keyboard.

A MIDI note-off starts the existing simulated key's return; it is not a measured
hardware position or an instruction for immediate damper contact. The key lifter holds
the damper clear until that coordinate reaches the assumed regulated contact point
(4.5 mm). Sustain and captured sostenuto support compose with the key lifter, so releasing
one control cannot defeat another. Release velocity controls key return and retains the
existing felt-seating rate proxy. There is no additional minimum hold or envelope.
After engagement, the grounded viscous brake and preload law are unchanged: this corrects
contact timing, not the post-contact modal decay law. Finite damper-head inertia and felt
compression mechanics are not included in this approximation.

## Unison tuning (synthetic voicing)

The existing Unison detune amount scales a fixed per-key profile in `make_grand`, not a
measured grand-piano tuning table. Two-string widths and three-string widths/asymmetries
vary deterministically by key; they do not change randomly on a strike or rebuild. Cents
sum to zero within each group, preserving its geometric target-frequency center; mono
strings are unaffected and amount zero aligns the target pitches exactly.

Width varies by ±15%, with a gentle fourth-root taper above a **fixed 440 Hz** reference.
That reference does not follow a custom A4 setting. This limits the growth of high-register
beat rates without claiming constant beating or a perceptually exact pitch center. All
previews and baked samples use the actual `make_grand` string targets, not parallel UI
pitch formulas. This is an intentional voicing choice, not evidence of more realistic sound.

## Numerics

- Leapfrog in time. Explicit for strings and bare plate, implicit (theta = 1/4) for ribs and
  bridges, whose fast bending waves would otherwise force a coarse grid.
- The plate grid is the finest that a Gershgorin bound on the assembled operator allows
  (bound 3.93 against a limit of 4). Strings satisfy their closed-form condition. The sum of
  the parts is then stable, because the coupled energy is a sum of positive pieces.
- Felt contact uses the discrete gradient of the felt potential, which makes the contact
  energy-exact. The equations are solved by a damped Newton iteration (3 iterations in the
  representative contact test, up to 12 in the full-compass stress test).
- Each string's two grid coefficients are fitted so that the fundamental is exact and the
  partial series follows the continuous stiff string. The physical stiffness enters through
  the inharmonicity coefficient.
- Feed-forward sound is computed one time step behind the mechanics, delaying output by
  about 21 microseconds. The separate air-loading force does feed back into the board.
- The string stencil updates interior nodes only. The bridge endpoint follows the board;
  its sampled increment is retained explicitly for the damper sleep decision.

## What the tests show

| Check | Result |
|---|---|
| Fast plate kernel equals the energy-derived operator | 5.3e-16 relative |
| Energy through a hammer strike, losses off | drift 8.6e-14 |
| Energy in free vibration, losses off, 2 s | drift 1.2e-12 |
| Energy with losses on (strike, hold, release) | never rises |
| Reciprocity, string to string through the board | 4.7e-14 |
| Plate modes against the analytic orthotropic plate | within 0.62 % (0.32 % at 2x rate) |
| Simulated partials against the scheme's closed form | 0.00 cents |
| Grid error of partials against the continuous string, C4 | about 3 cents to 4.4 kHz, -7 cents at 7 kHz, -33 cents at 10 kHz |
| Same, C6 | -10 cents at 9 kHz, -67 cents at 14 kHz |
| Felt pulse, C4 at 2 m/s | 8.38 N per string, 2.40 ms; peak moves 0.1 % from 48 to 96 kHz |
| Board drains the strings | 11.9 dB extra decay over 1.6 s relative to rigid supports |
| Helper threads | output bit-identical to one thread |
| Room | direct sound arrives on time; decay 0.81 s against Sabine 0.89 s |
| Longitudinal motion | tension pulses at twice the fundamental; longitudinal mode at 2402 Hz against 2413 Hz expected |
| Air loading | lowest resonance 43.8 Hz in vacuum, 40.4 Hz in air; fortissimo with pedal rings down |
| Action | hammer reaches the string 7.6, 1.0 and 0.4 ms after let-off at 0.3, 2 and 5 m/s |
| Pedal stir | 53 dB SPL against 79 dB SPL for one mezzo forte note, from silence at -146 dB |
| Audio Unit, driven in-process as a host would | 11 checks pass (formats, MIDI, own buffers, slice limit, state, 42 parameters, rebuild on change) |
| CLAP plug-in, driven in-process as a host would | 43 parameters, editor capability/size, sample-timed notes, pedal isolation across parameter changes, 48→96 kHz reactivation |
| Plugin rebuild lifecycle, real engine tests | stale sample-rate builds rejected; matching geometry/audio swap; source-owned pedals and sostenuto capture preserved |
| Boundary and scheduling regressions | prescribed endpoints, history-ring wrapping, and stereo/state equality with 0 vs 3 helpers through retriggers and pedals |
| Fortissimo abuse over the full compass with pedal | finite, bounded |

Measured, not tested against a target: bridge driving-point impedance 300 to 3000 kg/s across
the compass; lowest board resonances 44, 73, 107 Hz; a C4 falls 14 dB in the first second and
about 2 dB per second after.

## Processor budget

Each sample has one shared pass across the audio thread and three helper threads. Strings
without an active hammer advance immediately after their forces are computed. Hammer-contact
strings advance after the serial contact solve. Helpers spin only while a block is being
rendered. Their output matches the single-thread result bit for bit.

The engine accepts realtime-workgroup hooks. The Rust standalone wrapper does **not** expose
the selected device's workgroup, so its helpers currently do not join one. Do not infer
real-device scheduling performance from the offline benchmark.

### M4 Max offline measurements

Development measurements use M4-native builds at 48 kHz, board grid 1.5, with the full
model enabled. Portable release builds target the M1 instruction set; these numbers are
not a performance guarantee for every Apple Silicon Mac:

| Measurement | Result and scope |
|---|---|
| One-pass string scheduling | 109,120–109,255 cycles/sample versus 119,329–119,564 for the corrected two-pass reference: about 8.6 % less work in alternating-order runs of the Liszt demo |
| Single-thread pedalled benchmark | Median block deadline use 158–159 % versus about 171 % for the two-pass reference; still too slow for one core |
| Three helpers, pedalled benchmark | Median 74–93 %, p99 99–108 % in short runs on a shared machine; occasional missed deadlines remain |
| Flattened air-history indexing | About 4.8 % less time in an isolated `Radiation::step` benchmark; whole-engine gain not established |
| Fresh, unexcited instrument, no helpers | Median block deadline use 0.1 %, versus 78.8–79.0 % with ordinary stepping |
| Fresh, unexcited instrument, three helpers | Median 0.1–0.2 %, versus 50.5–50.6 % with ordinary stepping |

The one-pass and air-index optimizations preserve the corrected reference output exactly,
including a full 16-second stereo phrase with note releases. The string-boundary correctness
fix is separate: it changes when damped strings sleep and therefore changes some release
tails relative to the version with the out-of-bounds stencil read.

The pristine-idle figures come from three alternating runs per variant at 128 frames,
grid 1.5 and one second per scenario. They measure wall-clock block deadline use, not the
sum of CPU time across helper threads. The first block still executes one genuine tick and
its zero-state check: measured maxima including that work were 2.8 % without helpers and
4.7 % with three. A finer-grid cold snapshot took about 58 µs for tick plus check versus
17 µs for an ordinary tick; that one-time check is a real cost, not a timing guarantee.
Active cases showed no clear improvement or regression from this idle optimization.

### Exact pristine-idle path

An eligible new instrument executes one ordinary sample, then checks that its mechanical,
coupling, solver, output-history and acoustic/filter state is exactly zero. Nonzero and
nonfinite results reject the fast path. Constructors with awake dampers or damped-string
ringing enabled are excluded: damper preload can create sound without any key press.

For a proven zero state, only the delay clocks, buffer parity and ordinary housekeeping
advance; the process/oversampling/FIR loop is unchanged. Any excitation, changed pedal state
or mutable state-slice exposure disables the shortcut permanently for that instance.
No-op initialization setters retain eligibility. The shortcut never detects tails, never
re-arms after note-off, and never freezes an instrument merely because output is muted.
Tests compare exact sample/state bits across silent lead-ins, delay wraps, oversampling,
helper configurations, pedals, retriggers and mutable-state access.

Cycle counts remain sensitive to core placement, cache contention and power management.
Use alternating comparisons on the same machine, and use `bench` for wall-clock deadlines.
These are not real-device or sustained thermal measurements. A coarser board grid and the
Light preset reduce work, but also change the model; neither is counted as a quality-preserving
optimization.

### Precision and GPU

Double precision remains the default. A measured single-precision soundboard variant saved
about 27 % but introduced relative noise around -55 dB in the 16–24 kHz band. Single-precision
strings also changed tuning by a few hundredths of a cent. Neither substitution meets the
exact-output requirement.

`make build/metalbench && ./build/metalbench` measures synthetic float32 5-/7-point stencils,
not the double-precision orthotropic plate, stiffeners or coupled piano. M4 Max results:

| Workload | GPU, batched time per step | One CPU core |
|---|---|---|
| Board-sized 64×58 update | 6.2 µs | 0.8 µs |
| 256×256 update | 3.9 µs | 13.1 µs |
| 128³ air volume (2.10 million cells) | 42.7 µs | 577.1 µs |
| 21-million-cell room | 761.0 µs | 5,380.1 µs |

An empty GPU command-buffer commit/wait costs at least 16 µs in this run. That is a separate
lower-bound synchronization measurement, not a measured CPU/GPU coupled step. The instrument
exchanges forces every 20.8 µs; its small board is cheaper on the CPU even before that overhead.
The large-room GPU stencil alone is about 36.5 times over the per-sample budget. GPU batching
could suit independent offline renders, but no GPU baking implementation or equivalence proof
exists. Peak throughput is not a realtime-latency guarantee.

## Not modelled, or guessed

- **Sideways motion** is followed only for strings that have been struck; the faint sideways
  answer of other strings to the bridge rocking is left out. The hammer's strike angle is
  assumed.
- **Longitudinal motion** is one-way and its link to the board is a single downbearing
  angle (1.5 degrees, assumed).
- **Air.** Loading is resolved to quarter-metre patches, so it is a low-frequency model.
  Diffraction around the case and the lid's shadow are missing. The late reverberant field
  is statistical, not simulated.
- **Downbearing and prestress** of the board. The crown is included only as its membrane stiffness.
- **Shear and rotary inertia** in board and strings (thin-plate and Euler-Bernoulli limits).
- **Front duplex lengths, agraffe and pin compliance, the iron plate.** Rear duplex lengths
  are ideal thirds and halves, not a maker's scaling.
- **Action before let-off.** The key, wippen, jack and repetition lever accelerating the
  hammer are not simulated: with MIDI input their result, the hammer's speed at let-off, is
  what a note-on carries (hammer speed is a fixed curve of MIDI velocity). A note-off starts
  the simulated key return described above, not measured hardware release motion.
  Felt constants of the key rails and the
  backcheck, the key's mass, the share of their force that reaches the board, and the soft
  pedal's felt hardness are assumptions. Pedal friction is a noise source, not a friction model.
- **Geometry.** A generic 2 m grand: outline, scale, rib and bridge sizes are plausible
  values, not measurements of an instrument.
- **Material and felt data.** Spruce and maple constants are textbook values. Felt stiffness
  and exponent are interpolated from three published notes. Felt damping, damper strength,
  board loss factor, crown radius, action knock strength, pedal thresholds, room size and
  absorption are assumptions. The pedal thresholds were inferred from recorded pedalling.
- **Listening.** Checks are numerical. Judgements of tone so far are the owner's, by ear.

## Plug-ins

The Audio Unit (`aumu` / `FtAr` / `Odfk`) exposes physical settings through its host's generic
controls. CLAP and optional VST3 share the standalone app's integrated egui editor and also
expose board-grid choice. AU and CLAP pass in-process host tests; CLAP additionally checks
pedal behavior and reactivation at a different sample rate without opening an audio device.
Deterministic real-engine tests establish accepted rebuild and stale-generation behavior;
the external host test's parameter event alone does not prove a background rebuild completed.

Real-DAW editor embedding, real MIDI/audio-device behavior and Apple's `auval` remain
unverified. VST3 builds, but has not been exercised in a host; its bindings require GPL binary
distribution (see the README). No audio-device or microphone access is required by `make test`.
