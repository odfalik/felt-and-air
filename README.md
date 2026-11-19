# Felt & Air

A piano simulated from its mechanics, in real time on a Mac: stiff strings moving in two
directions and stretching along their length, felt hammers on springy shanks, an action,
dampers and three pedals, a ribbed anisotropic soundboard with bridges, the air on the
board, and a room. No samples, no filter bank standing in for the instrument.

It is a working instrument and a research prototype. It is **not** a validated model of a
whole piano. [docs/MODEL.md](docs/MODEL.md) says what is modelled, how it was checked, and
what is missing or guessed.

## Build and run

Needs Rust (cargo) and the Xcode Command Line Tools. The plug-in/app crate pins Rust 1.85.0;
its editor uses nih-plug and egui. Builds use at most four cargo jobs by default.
See [BUILDING.md](BUILDING.md) for exact offline toolchains and
[release packaging](docs/RELEASING.md) for controlled Apple Silicon builds and source/notices obligations.

```bash
make                 # engine, app, Audio Unit, CLAP plug-in, test hosts
make test            # numerical, plugin/MIDI routing and offline AU/CLAP host tests
open build/FeltAndAir.app
```

The app starts silent. Press **Return** or the Sound button to turn sound on.
A fresh, unexcited instrument avoids unnecessary mechanical work after an exact zero-state
check. Once played, it continues the full simulation, including its tails; muting is not a
CPU-suspension switch. See [measured performance and limits](docs/MODEL.md#processor-budget).

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

The app and CLAP/VST3 editor share one resizable window: live soundboard displacement,
bridge/string markers, last-note string shapes and felt forces, an 88-key keyboard, demo
transport, volume, board grid, load and over-budget counters. Four tabs (Space, Hammers,
Strings, Soundboard) contain every physical parameter, units and hover help. The Space
plan lets you drag the piano and listener. Presets and saved setups use the same controls.
Setups are stored in `~/Library/Application Support/Felt & Air/setups.json`.
Parameter changes are debounced and rebuild the instrument in the background; demo time
and held keys carry over. The Parts strip exposes existing physics switches; additional
whole-instrument switches and studio microphone pickup modes are not implemented.

To inspect the UI without any audio or MIDI device:

```bash
build/FeltAndAir.app/Contents/MacOS/FeltAndAir --backend dummy
```

Normal app launch uses CoreAudio output and watches MIDI sources for connection changes
about every half second. Existing connections are preserved; removing a keyboard releases
only its notes and pedal contributions. An explicit `--midi-input` selects a single input
through the host wrapper instead of automatic discovery. Audio input is unsupported.
Joining the selected CoreAudio device's realtime workgroup is not implemented in this shell.
MIDI routing/disconnect logic is tested without devices; physical hot-plug behavior remains
unverified. Demo files remain external, never included in the app; set
`FELT_AND_AIR_DEMOS` to relocate their directory.

## Plug-ins

```bash
make install-au      # Audio Unit  -> ~/Library/Audio/Plug-Ins/Components
make install-clap    # CLAP        -> ~/Library/Audio/Plug-Ins/CLAP
make vst3            # VST3 bundle in build/ (see the licence note below)
```

Every physical setting is a host parameter in both plug-ins; changing one rebuilds the
instrument in the background. CLAP and VST3 have the integrated egui editor; the Audio Unit
uses its host's generic parameter controls. CLAP/VST3 also expose board grid as a parameter.
Release builds explicitly disable symbol stripping because Rust 1.85's debuginfo stripping
can produce a Mach-O string table that macOS 27's dynamic loader rejects.

The pinned `vst3-sys` bindings grant GPLv3-or-later. The original project sources remain
MIT; a combined VST3 binary is distributed on GPLv3-compatible terms, with matching source
and dependency/font notices. It is not built by default and has not been run in a real DAW.

## Tools

```bash
./engine/target/release/render out/phrase.wav phrase     # offline render of a short phrase
./engine/target/release/render out/x.wav demos/01.mid    # ... or of a MIDI file
./engine/target/release/cycles demos/05.mid              # work per time step, in processor cycles
./engine/target/release/bench                            # share of each block's deadline used
./engine/target/release/report                           # bridge impedance across the compass
./build/metalbench                                       # GPU against CPU for this kind of work
python3 tools/compare_wav.py a.wav b.wav                 # level of the difference between two renders
```

## Layout

- `engine` the instrument, a Rust crate with no dependencies; `engine/include` its C interface
- `engine/tests` validation suite
- `plugin` shared Rust Mac app and CLAP (optional VST3) plug-in, on nih-plug/egui
- `src/app` standalone app bundle metadata
- `src/au` Audio Unit
- `src/shared` the Audio Unit's C++ wrapper around the engine's C interface

## Optional external demos

Demo files are optional and never packaged in the app. The app and plug-ins look in
`~/Library/Application Support/Felt & Air/demos`, or in `FELT_AND_AIR_DEMOS` when
set at run time to a nonempty directory path. The directory contains `demos.txt` and the
MIDI files named in it. Missing demos are harmless; no directory is created or downloaded.
There is no compile-time checkout path or implicit current-working-directory lookup.

For demos fetched into a checkout, launch with:

```bash
FELT_AND_AIR_DEMOS="$PWD/demos" build/FeltAndAir.app/Contents/MacOS/FeltAndAir
```

Finder launches use the per-user directory unless their environment supplies the override.

## Licence

Original project source: MIT. Dependencies, Rust runtime components, and bundled fonts retain
their own licences; a GUI binary is not an MIT-only distribution. See the VST3 note above and
[release licence/source requirements](docs/RELEASING.md#assets-and-licenses).
The demo recordings fetched by `tools/fetch_demos.py` are not part of this repository and
carry their own licence (CC BY-NC-SA 4.0).
