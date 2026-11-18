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
make test       # validation suite (about 40 s)
open build/PhysicalPiano.app
```

The app starts silent. Press **Return** to turn sound on.

| Keys | Action |
|---|---|
| `A W S E D F T G Y H U J K O L P ;` | play (one octave and a bit) |
| `Z` / `X` | octave down / up |
| `C` / `V` | softer / harder |
| `Space` | sustain pedal |
| `1` `2` `3` | soundboard grid: finest, 1.25x, 1.5x spacing |

MIDI keyboards are picked up automatically. The mouse plays the on-screen keys.

## Tools

```bash
./build/render out/phrase.wav phrase     # offline render of a short phrase
./build/render out/c4.wav c4 2           # one note at 2x internal rate
./build/bench                            # processor budget per scenario
python3 tools/compare_wav.py a.wav b.wav # level of the difference between two renders
```

## Layout

- `src/core` engine, plain C++20, no dependencies
- `src/app` standalone app (AppKit, CoreAudio, CoreMIDI)
- `src/tools` offline renderer and benchmark
- `tests` validation suite
