# Physical Piano

A piano simulated from its mechanics, in real time on a Mac: stiff strings, felt hammers,
dampers, a ribbed anisotropic soundboard with bridges, and sound radiated from the moving
board. No samples, no filter bank standing in for the instrument.

It is a working instrument and a research prototype. It is **not** a validated model of a
whole piano. [docs/MODEL.md](docs/MODEL.md) says what is modelled, how it was checked, and
what is missing or guessed.

## Build and run

Needs only the Xcode Command Line Tools.

```bash
make            # engine, tests, tools, app
make test       # validation suite (about a minute)
make install-au # Audio Unit into ~/Library/Audio/Plug-Ins/Components
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

Controls in the window: sound on/off, volume, a menu of demo recordings, board grid, and an
**Instrument** window with four tabs (Space, Hammers, Strings, Soundboard): sliders in
physical units, switches for each part of the physics, presets, your own saved setups, and a
room plan where the piano and the listener can be dragged. Changes rebuild the instrument
when you let go.

MIDI keyboards are picked up automatically. The mouse plays the on-screen keys.

## Tools

```bash
./build/render out/phrase.wav phrase     # offline render of a short phrase
./build/render out/c4.wav c4 2           # one note at 2x internal rate
./build/bench                            # processor budget per scenario
./build/report                           # bridge impedance across the compass
./build/metalbench                       # GPU against CPU for this kind of work
./build/PhysicalPiano.app/Contents/MacOS/PhysicalPiano --loadtest 0 30   # load on the real audio thread, silent
python3 tools/compare_wav.py a.wav b.wav # level of the difference between two renders
```

## Layout

- `src/core` engine, plain C++20, no dependencies
- `src/app` standalone app (AppKit, CoreAudio, CoreMIDI)
- `src/tools` offline renderer and benchmark
- `tests` validation suite

## Licence

MIT. The demo recordings fetched by `tools/fetch_demos.py` are not part of this repository
and carry their own licence (CC BY-NC-SA 4.0).
