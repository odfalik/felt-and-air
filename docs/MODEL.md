# Model, validation, and limits

Numbers are from `make test`, `./build/bench`, `./build/report`, `./build/metalbench` and the
app's `--loadtest` on a MacBook Air M3, 48 kHz, double precision.

## What is simulated

One discrete mechanical system, `M q'' + C q' + K q = f`, stepped in time. `K` is symmetric
and comes from a discrete strain energy, so every part exchanges energy with every other
part consistently.

| Part | Model |
|---|---|
| Strings | Stiff string (tension + bending), 228 strings for 88 keys, 5 to 200 grid nodes each. Optional second (sideways) polarisation |
| Stretching | Longitudinal motion on its own coarse grid, driven by the transverse wave; its tension change presses on the bridge through the downbearing angle. One-way: it does not act back on the transverse motion |
| Duplex | 99 free lengths of the treble wires behind the bridge, a third or half as long, never struck or damped |
| Tuning | A4 = 440 Hz, octaves tuned outward by matching partials (4:2, 2:1 in the high treble, 6:3 in the bass). Gives -14 cents at A0 and +34 cents at C8 |
| String loss | Constant plus frequency-squared decay rate. The constant is 0.12 /s, not the 0.5 /s of Chaigne and Askenfelt (1994), which was fitted with a rigid bridge and so contained the soundboard's share |
| Hammer | Head on a flexible shank (two masses and a spring), nonlinear felt `F = K z^p` with Hunt-Crossley damping, one hammer striking all strings of its note, in free flight as after let-off |
| Keys | A force pulse on the board's front edge when the key lands, a softer one when it returns; release speed sets how fast the damper comes down |
| Pedal work | A knock at each end of the pedal's travel and a faint friction noise while it moves, as forces on the case |
| Dampers | Felt that rests on the string with a small force and soaks up its motion. Lifting it releases the slightly deflected string, which is the stir of the strings when the pedal goes down (about 35 dB below a mezzo forte note). Damped strings stay in the simulation. The pedal is continuous: dampers graze the strings between 34 % and 47 % of its travel |
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
| Audio Unit, driven in-process as a host would | 9 checks pass (formats, MIDI notes and pedal, own buffers, slice limit, state) |
| Fortissimo abuse over the full compass with pedal | finite, bounded |

Measured, not tested against a target: bridge driving-point impedance 300 to 3000 kg/s across
the compass; lowest board resonances 44, 73, 107 Hz; a C4 falls 14 dB in the first second and
about 2 dB per second after.

## Processor budget

The work of one time step is shared between the audio thread and three helper threads that
join the audio device's realtime thread group and spin only while a block is being rendered.

On the real audio thread, output silenced, board grid 1.5x (1,632 nodes), room on, every
part of the model above on except the sideways polarisation, machine otherwise lightly loaded:

| Material | Mean load | Worst block after the first |
|---|---|---|
| Bach prelude | 70 to 72 % | 81 % |
| Chopin nocturne, pedalled | 66 to 73 % | 81 % |

The two cost the same because damped strings are now always simulated: 327 strings are awake
at all times. The first one or two blocks after sound starts run over budget. Before
longitudinal motion, air loading, duplex lengths and always-awake strings were added, the
same cases cost 45 % and 56 %. Finer board grids cost more. Sustained thermal behaviour on
the fanless Air has not been measured.

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

- **Sideways polarisation.** Implemented and energy-exact, but its coupling to the board is a
  tilted bridge axis with assumed angles, and with them it changes the level by at most
  2.5 dB late in a note. It is off by default. A coupling derived from the bridge rocking on
  the board, with no free angle, has not been built.
- **Longitudinal motion** is one-way and its link to the board is a single downbearing
  angle (1.5 degrees, assumed).
- **Air.** Loading is resolved to quarter-metre patches, so it is a low-frequency model.
  Diffraction around the case and the lid's shadow are missing. The late reverberant field
  is statistical, not simulated.
- **Downbearing and prestress** of the board. The crown is included only as its membrane stiffness.
- **Shear and rotary inertia** in board and strings (thin-plate and Euler-Bernoulli limits).
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

## Plug-in

`make au` builds an Audio Unit instrument (`aumu` / `PhPn` / `Odfk`) with one parameter
(volume), MIDI notes, release velocity and a continuous sustain pedal. `make install-au`
copies it to `~/Library/Audio/Plug-Ins/Components`. It has no editor window, so the
Instrument settings are those of the reference grand. Apple's `auval` has not been run on
it: the system's component registry had not picked the new plug-in up, and refreshing it
needs a log-out or administrator rights. There is no VST3 build; that needs Steinberg's SDK.
