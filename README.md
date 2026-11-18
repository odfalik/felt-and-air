# Physical Piano

A piano simulated from its mechanics, in real time on a Mac: stiff strings moving in two
directions and stretching along their length, felt hammers on springy shanks, an action,
dampers and three pedals, a ribbed anisotropic soundboard with bridges, the air on the
board, and a room. No samples, no filter bank standing in for the instrument.

It is a working instrument and a research prototype. It is **not** a validated model of a
whole piano. [docs/MODEL.md](docs/MODEL.md) says what is modelled, how it was checked, and
what is missing or guessed.

## Build and run

Needs Rust (cargo) and the Xcode Command Line Tools.

```bash
make                 # engine, app, Audio Unit, CLAP plug-in, test hosts
make test            # validation suite (about a minute) and the two plug-in host tests
open build/PhysicalPiano.app
```

The app starts silent. Press **Return** or the Sound button to turn sound on.

For the demo recordings (real performances, 58 MB download, CC BY-NC-SA):

```bash
python3 -I tools/fetch_demos.py
```

| Keys | Action |
|---|---|
| `A W S E D F T G Y H U J K O L P ;` | play (one octave and a bit) |
| `Z` / `X` | octave down / up |
| `C` / `V` | softer / harder |
| `Space` | sustain pedal |
| `,` / `.` | soft pedal / sostenuto |

MIDI keyboards are picked up automatically, with release velocity and a continuous sustain
pedal. The mouse plays the on-screen keys.

Controls in the window: sound on/off, volume, a menu of demo recordings, board grid, and an
**Instrument** window with four tabs (Space, Hammers, Strings, Soundboard): sliders in
physical units, switches for each part of the physics, presets, your own saved setups, and a
room plan where the piano and the listener can be dragged. Changes rebuild the instrument
when you let go.

## Plug-ins

```bash
make install-au      # Audio Unit  -> ~/Library/Audio/Plug-Ins/Components
make install-clap    # CLAP        -> ~/Library/Audio/Plug-Ins/CLAP
make vst3            # VST3 bundle in build/ (see the licence note below)
```

Every setting of the Instrument window is a host parameter in both plug-ins; changing one
rebuilds the instrument in the background. Neither has its own editor window.

The VST3 build goes through the `vst3-sys` bindings, which are GPLv3. This repository is
MIT, but a VST3 binary built from it has to be distributed under the GPL. It is not built
by default and has not been run in a host.

## Tools

```bash
./engine/target/release/render out/phrase.wav phrase     # offline render of a short phrase
./engine/target/release/render out/x.wav demos/01.mid    # ... or of a MIDI file
./engine/target/release/cycles demos/05.mid              # work per time step, in processor cycles
./engine/target/release/bench                            # share of each block's deadline used
./engine/target/release/report                           # bridge impedance across the compass
./build/metalbench                                       # GPU against CPU for this kind of work
./build/PhysicalPiano.app/Contents/MacOS/PhysicalPiano --loadtest 0 30   # load on the real audio thread, silent
python3 tools/compare_wav.py a.wav b.wav                 # level of the difference between two renders
```

## Layout

- `engine` the instrument, a Rust crate with no dependencies; `engine/include` its C interface
- `engine/tests` validation suite
- `plugin` CLAP (and optional VST3) plug-in, on nih-plug
- `src/app` the Mac app (AppKit, CoreAudio, CoreMIDI)
- `src/au` Audio Unit
- `src/shared` the C++ face on the engine's C interface that app and Audio Unit share

## Licence

MIT. The demo recordings fetched by `tools/fetch_demos.py` are not part of this repository
and carry their own licence (CC BY-NC-SA 4.0). See the note on VST3 above.
