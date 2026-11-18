# Model, validation, and limits

Numbers are from `make test`, the tools listed in the README and the app's `--loadtest`, on a
MacBook Air M3 at 48 kHz in double precision. The engine is Rust; an earlier C++ version of
it produced the same output to within the precision of a 32 bit WAV file (-155 dB).

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
| Action after let-off | The hammer is released 2 mm under the string and flies there under gravity (7.6 ms at pianissimo, 1.0 ms at mezzo forte, 0.4 ms at fortissimo), comes back and is caught on the backcheck felt, then falls to rest when the key is released. The key lands on its rail felts going down and coming up. Felts are stiffening springs with drag; a share of their changing force reaches the board |
| Pedals | Sustain is continuous. Soft (una corda) makes each hammer miss one string of three, on softer felt. Sostenuto holds up the dampers of the keys down when it is pressed. The pedal work knocks at each end of its travel and rustles faintly while moving |
| Dampers | Felt that rests on the string with a small force and soaks up its motion. Lifting it releases the slightly deflected string, which is the stir of the strings when the pedal goes down (26 dB below a mezzo forte note). A settled damped string is set aside and wakes in its bent shape; optionally damped strings stay in the simulation and answer faintly to the rest. The pedal is continuous: dampers graze the strings between 34 % and 47 % of its travel |
| Soundboard | Orthotropic Kirchhoff plate on a grain-aligned grid, grand-shaped outline, clamped rim, crown as a shallow-shell stiffness |
| Air on the board | Each quarter-metre patch feels its own motion as a piston (mass-like at low, resistive at high frequency) and the delayed acceleration of every other patch, on both faces. Lowers the lowest resonance by 8 % |
| Ribs | Beams across the grain, with a composite-section stiffness and tapered ends |
| Bridges | Beams along the bass and treble bridge lines, tied to the plate by interpolation |
| String to bridge | The string's end node is the plate at that point. Force and motion pass both ways with no extra parameters |
| Radiation | Each side of the board radiates into its own half space with opposite sign, summed node by node to two ears |
| Room | Floor, open lid, walls and ceiling as mirror images up to second order over quarter-metre board patches; a late field from a feedback delay network scaled to Sabine's decay time and level |

## Numerics

- Leapfrog in time. Explicit for strings and bare plate, implicit (theta = 1/4) for ribs and
  bridges, whose fast bending waves would otherwise force a coarse grid.
- The plate grid is the finest that a Gershgorin bound on the assembled operator allows
  (bound 3.93 against a limit of 4). Strings satisfy their closed-form condition. The sum of
  the parts is then stable, because the coupled energy is a sum of positive pieces.
- Felt contact uses the discrete gradient of the felt potential, which makes the contact
  energy-exact. The equations are solved by a damped Newton iteration (at most 4 iterations seen).
- Each string's two grid coefficients are fitted so that the fundamental is exact and the
  partial series follows the continuous stiff string. The physical stiffness enters through
  the inharmonicity coefficient.
- Sound is computed one time step behind the mechanics. It does not act back on the
  instrument, so this only delays the output by 21 microseconds.

## What the tests show

| Check | Result |
|---|---|
| Fast plate kernel equals the energy-derived operator | 4e-16 relative |
| Energy through a hammer strike, losses off | drift 3e-14 |
| Energy in free vibration, losses off, 2 s | drift 2e-13 |
| Energy with losses on (strike, hold, release) | never rises |
| Reciprocity, string to string through the board | 3e-14 |
| Plate modes against the analytic orthotropic plate | within 0.6 % (0.3 % at 2x rate) |
| Simulated partials against the scheme's closed form | 0.00 cents |
| Grid error of partials against the continuous string, C4 | under 3 cents to 4.4 kHz, -8 cents at 7 kHz, -36 cents at 10 kHz |
| Same, C6 | -10 cents at 9 kHz, -69 cents at 14 kHz |
| Felt pulse, C4 at 2 m/s | 10.1 N, 2.0 ms; peak moves 0.5 % from 48 to 96 kHz |
| Board drains the strings | 4 dB extra decay over 1.6 s |
| Helper threads | output bit-identical to one thread |
| Room | direct sound arrives on time; decay 0.81 s against Sabine 0.89 s |
| Longitudinal motion | tension pulses at twice the fundamental; longitudinal mode at 2402 Hz against 2413 Hz expected |
| Air loading | lowest resonance 44.2 Hz in vacuum, 40.7 Hz in air; fortissimo with pedal rings down |
| Action | hammer reaches the string 7.6, 1.0 and 0.4 ms after let-off at 0.3, 2 and 5 m/s |
| Pedal stir | 53 dB SPL against 79 dB SPL for one mezzo forte note, from silence at -146 dB |
| Audio Unit, driven in-process as a host would | 11 checks pass (formats, MIDI, own buffers, slice limit, state, 42 parameters, rebuild on change) |
| CLAP plug-in, driven in-process as a host would | 6 checks pass (descriptor, 42 parameters, notes in, sound out) |
| Fortissimo abuse over the full compass with pedal | finite, bounded |

Measured, not tested against a target: bridge driving-point impedance 300 to 3000 kg/s across
the compass; lowest board resonances 44, 73, 107 Hz; a C4 falls 14 dB in the first second and
about 2 dB per second after.

## Processor budget

The work of one time step is shared between the audio thread and three helper threads that
join the audio device's realtime thread group and spin only while a block is being rendered.
Output is bit-identical to the single-thread result.

Work per step on one thread, in processor cycles, which do not depend on how busy the
machine is: about 121,000 for a pedalled virtuoso piece (Liszt, La Campanella) at board grid
1.5x with everything on. One sample at 48 kHz allows 84,000 on one performance core, so one
thread alone cannot do it; sideways motion accounts for 5 % of that work.

On the real audio thread, output silenced, same settings, on a machine that was also busy
with other work (load average about 5):

| Material | Mean load | Worst blocks |
|---|---|---|
| Bach prelude | 65 to 76 % | about 100 % |
| Chopin nocturne, pedalled | 82 to 88 % | 103 to 107 %, a few per cent of blocks over budget |

That is too little headroom on a busy machine: expect occasional dropouts in pedalled
passages. The "Light" preset (no sideways motion, duplex or air loading) and a coarser board
grid buy room. Timings taken minutes apart on this machine differed by a factor of two to
three, so these figures are a range, not a constant. Sustained thermal behaviour on the
fanless Air has not been measured separately.

**Single precision** was tried and rejected for now. For the soundboard it saves about 27 %
but adds noise at -55 dB relative to the signal in the 16 to 24 kHz band. For the strings
alone the noise is lower (-81 dB in 8 to 16 kHz) but rounding of the stencil coefficients
detunes each string by a few hundredths of a cent, which changes unison beating.

**Metal** was measured rather than assumed:

| Workload | GPU | One CPU core |
|---|---|---|
| Board-sized 2D update, per step | 6.4 us (plus a 12 us round trip if the CPU needs the result every step) | 0.9 us |
| 3D air, 2 million cells, per step | 292 us | 649 us |
| 3D air, throughput | about 7 billion cell updates per second | 2 to 3.7 billion |

So the GPU loses for the coupled instrument, which must exchange data every 20.8 us, and
wins by about 2.5x for a volume of air. That is still far short of a full-band room: a
4 x 3.2 x 3.2 m room at 12 mm needs 21 million cells and 3 ms per step, 146 times over
budget. A real-time air simulation on this GPU is limited to roughly a 7 m room below
1.5 kHz, with nothing left over. The room model above is geometric for that reason.

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
  what a note-on carries (hammer speed is a fixed curve of MIDI velocity), and a note-off is
  taken as the moment the damper meets the string. Felt constants of the key rails and the
  backcheck, the key's mass, the share of their force that reaches the board, and the soft
  pedal's felt hardness are assumptions. Pedal friction is a noise source, not a friction model.
- **Front duplex lengths, agraffe and pin compliance, the iron plate.** Rear duplex lengths
  are ideal thirds and halves, not a maker's scaling.
- **Action.** The key, wippen, jack and repetition lever are not simulated as bodies. With
  MIDI input their result, the hammer's speed and timing at let-off, is what the note-on
  already carries; hammer speed is a fixed curve of MIDI velocity. Shank flex, key and pedal
  knocks and damper mechanics are modelled, with guessed strengths. Pedal friction is a
  noise source, not a friction model. Una corda and sostenuto pedals are missing.
- **Geometry.** A generic 2 m grand: outline, scale, rib and bridge sizes are plausible
  values, not measurements of an instrument.
- **Material and felt data.** Spruce and maple constants are textbook values. Felt stiffness
  and exponent are interpolated from three published notes. Felt damping, damper strength,
  board loss factor, crown radius, key knock strength, pedal thresholds, room size and
  absorption are assumptions. The pedal thresholds were inferred from recorded pedalling.
- **Listening.** Checks are numerical. Judgements of tone so far are the owner's, by ear.

## Plug-ins

An Audio Unit (`aumu` / `PhPn` / `Odfk`) and a CLAP plug-in, both with every setting as a
host parameter and neither with an editor window of its own. Both pass a host-style test
run in-process. Apple's `auval` has not been run: the system's component registry had not
picked the new Audio Unit up, and refreshing it needs a log-out or administrator rights.
Neither has been loaded in a real host. A VST3 build exists behind a switch, untested, and
its bindings are GPL (see the README).
