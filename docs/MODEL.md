# Model, validation, and limits

State as of the first milestone. Numbers are from `make test` and `./build/bench` on a
MacBook Air M3, 48 kHz, double precision.

## What is simulated

One discrete mechanical system, `M q'' + C q' + K q = f`, stepped in time. `K` is symmetric
and comes from a discrete strain energy, so every part exchanges energy with every other
part consistently.

| Part | Model |
|---|---|
| Strings | Stiff string (tension + bending), both transverse polarisations, 228 strings for 88 keys, grid of 5 to 200 nodes each |
| Polarisation coupling | The bridge yields along one tilted axis, so upright and sideways motion share it. The hammer strikes slightly off the upright |
| String loss | Constant plus frequency-squared decay rate (Chaigne and Askenfelt 1994 form). The constant is 0.12 /s, not their 0.5 /s, which was fitted with a rigid bridge and so already contained the soundboard's share |
| Hammer | Point mass, nonlinear felt `F = K z^p` with Hunt-Crossley damping, one hammer striking all strings of its note |
| Dampers | Viscous felt over a short length of string, lifted by key or pedal |
| Soundboard | Orthotropic Kirchhoff plate on a grain-aligned grid, grand-shaped outline, simply supported rim |
| Ribs | Beams across the grain, with a composite-section stiffness and tapered ends |
| Bridges | Beams along the bass and treble bridge lines, tied to the plate by interpolation |
| String to bridge | The string's end node is the plate at that point. Force and motion pass both ways with no extra parameters |
| Sound | Rayleigh integral: every board node radiates to two listening points with its own delay |

## Numerics

- Leapfrog in time. Explicit for strings and bare plate, implicit (theta = 1/4) for ribs and
  bridges, whose fast bending waves would otherwise force a coarse grid.
- The plate grid is the finest that a Gershgorin bound on the assembled operator allows
  (bound 3.93 against a limit of 4). Strings satisfy their closed-form condition. The sum of
  the parts is then stable, because the coupled energy is a sum of positive pieces.
- Felt contact uses the discrete gradient of the felt potential, which makes the contact
  energy-exact. The resulting equations are solved by a damped Newton iteration (at most 4
  iterations seen).
- Each string's two grid coefficients are fitted so that the fundamental is exact and the
  partial series follows the continuous stiff string. The physical stiffness enters through
  the inharmonicity coefficient.

## What the tests show

| Check | Result |
|---|---|
| Fast plate kernel equals the energy-derived operator | 5e-16 relative |
| Energy through a hammer strike, losses off | drift 3e-14 |
| Energy in free vibration, losses off, 2 s | drift 1e-13 |
| Energy with losses on (strike, hold, release) | never rises |
| Reciprocity, string to string through the board | 1e-14 |
| Plate modes against the analytic orthotropic plate | within 0.6 % (0.3 % at 2x rate) |
| Simulated partials against the scheme's closed form | 0.00 cents |
| Grid error of partials against the continuous string, C4 | under 3 cents to 4.4 kHz, -8 cents at 7 kHz, -36 cents at 10 kHz |
| Same, C6 | -10 cents at 9 kHz, -69 cents at 14 kHz |
| Felt pulse, C4 at 2 m/s | 10.1 N, 2.0 ms; peak moves 0.5 % from 48 to 96 kHz |
| Board drains the strings | 4 dB extra decay over 1.6 s |
| Fortissimo abuse over the full compass with pedal | finite, bounded |

## Processor budget

Counted in processor cycles, which do not depend on how busy the machine is. One sample at
48 kHz allows about 84,000 cycles on one performance core.

| Board grid | Nodes | Idle | Six-note chord | Pedal down, all strings |
|---|---|---|---|---|
| finest (30 x 15 mm) | 3,679 | 75 % | 79 % | 127 % |
| 1.25x (38 x 19 mm) | 2,354 | 54 % | 60 % | 107 % |
| 1.5x (45 x 23 mm) | 1,635 | 41 % | 48 % | 96 % |

These figures predate the second polarisation, which doubles the string work: at 1.25x a
six-note chord is now about 72 % and pedal down about 167 %. The app defaults to 1.25x. **Pedal down with every string ringing does not fit in one core
at any setting.** Sustained thermal behaviour on the fanless Air has not been measured.

Single precision was tried and rejected for now: it saves about 27 % but adds noise at
-55 dB relative to the signal in the 16 to 24 kHz band, coming from the soundboard update.

Metal has not been measured yet. The expectation, to be tested, is that it loses for the
per-sample coupled update (a few thousand nodes, 48,000 dispatches a second) and wins for a
3D air volume.

## Not modelled, or guessed

- **Longitudinal string motion.** No phantom partials.
- **Polarisation coupling strength.** Bridge tilt (6 degrees) and hammer tilt (4 degrees)
  are assumptions. With them the sideways motion is too weak to hear; the two-stage decay
  that is there comes from unison detuning. A C4 now falls 14 dB in the first second and
  about 2 dB per second after that.
- **Air.** No air loading on the board, no room, lid, case or floor. The baffled-plate
  integral over-radiates at low frequency; levels come out roughly 10 dB hot.
- **Soundboard crown, downbearing and prestress.**
- **Rim.** Simply supported; a real rim is closer to clamped.
- **Shear and rotary inertia** in board and strings (thin-plate and Euler-Bernoulli limits).
- **Duplex and back-string lengths, agraffe and pin compliance, the iron plate.**
- **Action.** The hammer is in free flight from a set speed. No key, escapement or shank flex.
- **Geometry.** A generic 2 m grand: outline, scale, rib and bridge sizes are plausible
  values, not measurements of an instrument.
- **Material data.** Spruce and maple constants are textbook values. Felt stiffness and
  exponent are interpolated from three published notes. Felt damping (0.1 s/m), damper
  strength, board loss factor (2 %) and the listening positions are assumptions.
- **Sleeping strings.** A fully damped string that has stopped moving is replaced by a
  dashpot at the bridge until it is played or the pedal lifts.
- **Listening.** Nothing here has been auditioned. All checks so far are numerical.

## Sympathetic resonance without the pedal

Modelled: the top 19 notes have no dampers and always ring; strings of held keys ring.
Not modelled: duplex and back-string lengths, and imperfect damping (a damped string here
is silent once it has settled). Bridge impedance measured in the model (`./build/report`)
is 400 to 2000 kg/s across the compass, in the range reported for real instruments.
