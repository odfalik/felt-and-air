# Bake a preset to SFZ

The offline exporter makes a **sampled approximation**, not another physical engine. It uses the same saved `Setup` (`values`, `grid`, `volume`) as the editor, a fresh full-compass piano for every key/layer, and the plugin's output gain/volume/tanh pipeline. It never normalizes individual WAVs or adds velocity gain after capturing the physical strike.

There is no Bake button yet. The reusable `bake` module is for a background worker, not an audio callback. The CLI opens no device and does not read or change a running app.

## Small end-to-end smoke

```sh
nice -n 15 cargo run --manifest-path plugin/Cargo.toml --release --locked --offline -j 1 \
  --bin felt-and-air-bake -- \
  --output "$PWD/out/my-bake-smoke" --defaults \
  --keys 60,64,67 --velocities 32,80,120 --sample-rate 48000 \
  --hold-seconds 0.25 --end-fade-seconds 0.02 --release-seconds 0.15
```

This makes nine quarter-second float32 stereo WAVs, `preset.sfz`, `manifest.json`, and a limitations README. It is only a C4/E4/G4 smoke bank, **not a full piano**. `--defaults` explicitly chooses reference defaults; it is not a claim to capture the current live app.

To bake a saved editor setup, replace `--defaults` with:

```sh
--setup-file "$HOME/Library/Application Support/Felt & Air/setups.json" --setup-name "My setup"
```

No alternative settings schema is introduced. The job owns a snapshot, and later live edits cannot change it. Corrupt/incompatible snapshots fail rather than silently using defaults. The manifest retains the exact captured setup and render settings. `--defaults` uses the same parameter capture as the editor, including FloatParam rounding/clamping, rather than raw `GrandOptions`; grid mapping is shared. JSON uses the existing dependency's `float_roundtrip` feature to preserve captured values exactly.

`source_revision` is the optional **compile-time** `FELT_AND_AIR_BUILD_REVISION` (full lowercase 40-hex Git SHA). Invalid values fail compilation; an unavailable revision is explicitly JSON `null`, not a guessed current checkout. The exporter never runs Git at runtime. Package version is recorded separately; a builder may supply the known source revision without introducing another build system.

## Explicit scope and resource bounds

Keys, velocity layers, rate, finite hold, end fade, and approximate sampler release are required. Keys accept comma lists or inclusive MIDI ranges; `--keys 21-108` explicitly requests the full keyboard, and can be expensive. MIDI velocity zero is rejected: it is NoteOff, not a quiet strike.

The plan reports WAV bytes and total engine-audio seconds before work, not an unmeasured wall-time prediction. The exporter uses zero helper threads and holds one stereo sample in memory at a time. Supported rates are 44.1/48/96 kHz; holds are configurable from 20 ms to 120 s, with at most 16 layers, 2 GiB of WAV data and one hour of engine-audio rendering per job. These are upper safety bounds, not a suggested overnight bank. Metadata needs additional disk space; write errors leave the bank incomplete.

Output-directory creation is exclusive: even an empty existing directory is refused. Errors, library cancellation, or CLI interruption retain `INCOMPLETE.txt`; do not import such a bank. The library checks its cancellation flag each 64-frame block and around each instrument construction/write. Construction and one WAV write must return before cancellation can be observed. No files are installed or existing banks deleted. The complete SFZ is written/synced to a staging file, then published atomically with a same-directory no-clobber hard link before the incomplete marker is removed. It never uses an overwriting rename; a final-name collision or a filesystem without hard-link support fails safely and keeps the incomplete marker.

## Approximation limits

- **Finite sustain, no loops.** Each held-key recording has an explicit end-only cosine fade to exact zero, never an attack fade or a hard audible last-sample cut. A hold longer than the file ends in that artificial fade. Unfaded final RMS and peak are recorded; the CLI warns when real decay was shortened. Choose longer samples when needed.
- **Release is a sampler envelope.** `ampeg_release` is an explicit approximation, not captured physical damper return or per-partial damping. The 150 ms smoke value is not a real-piano calibration. Sampler pedal behavior is host-dependent; re-pedalling, sostenuto and dynamic una-corda are not captured.
- **Discrete velocity layers.** Each region selects the nearest recorded velocity using non-overlapping midpoint bins, with `amp_veltrack=0`. Dynamics are baked, not normalized or multiplied a second time. There is no continuous physical response between layers.
- **Independent voices.** A fresh full-compass capture retains that note's board loading, room and other-string response, but combining samples cannot reproduce shared polyphonic coupling, retrigger phase, or the live nonlinear output bus. Baked room and per-note tanh do not equal rendering a live chord. Changed physical settings require rebaking.
- Float32 WAV/SFZ file checks are not proof of successful import or playback in a real SFZ host; that remains unverified. No sampler dependency or device is required to export.

## Verification

Real-engine tests create the nine-sample smoke through the exporter, inspect actual WAV headers/lengths/finite samples and all SFZ regions/bins, compare every pre-fade sample bit-for-bit with an independent full-compass engine capture through the same gain transform, and verify actual editor-default capture/rounding, immutable setup and exact JSON roundtrip, explicit optional source revision, zero file endpoints, cancellation mid-render/after one file, and output-directory/final-SFZ no-clobber behavior. They do not use mocked audio or establish real-host compatibility.
