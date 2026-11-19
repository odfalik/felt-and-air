# Building Felt & Air

Felt & Air is an experimental physical piano. Builds and tests below do not open an audio device. Do not launch the standalone app or a host as part of unattended verification.

## Toolchains

- Apple Silicon macOS with Xcode and its command-line tools.
- Rust **1.96.1** for `engine/`; Rust **1.85.0** for `plugin/`.
- Python **3.12+** for release tooling; no Python packages are required.
- `make`, `clang++`, `cargo`, `rustup`, `codesign`, `otool`, `lipo`, and `install_name_tool`.

The source checkout's normal development settings use the local CPU. They are **not release portability settings**. The release tool overrides both Rust and C++ settings with `apple-m1`, `aarch64-apple-darwin`, and a macOS **13.0 deployment target**. That deployment target is not evidence that the program has been runtime-tested on macOS 13. Published provenance identifies the actual test OS and SDK.

Normal developer commands:

```sh
make CARGO_JOBS=4 test
make CARGO_JOBS=4 app vst3
```

These do not install plug-ins. `install-au` and `install-clap` are separate, explicit operations. Do not run them for release verification.

## Offline source distributions

A release source archive includes the exact project snapshot, original lockfiles, all locked registry dependencies, pinned Git dependencies, relative Cargo source replacements, licenses, and release tooling. It excludes `.git`, build outputs, private build logs, and demo recordings.

The two pinned `baseview 0.1.0` Git revisions cannot share one `cargo vendor` directory. The release tool uses two temporary Cargo vendor probe graphs, verifies every external probe identity/checksum against the original lock, and stores Git packages in separate per-origin directory sources. All registry lock packages are included, not only packages used on this Mac. The final project manifests and lockfiles remain byte-for-byte unchanged. Cargo, not the release script, normalizes the Git package manifests.

With the required toolchains already installed, an extracted archive can build without a populated Cargo registry index or Git cache. The strict local verification command is:

```sh
python3 tools/release.py prepare --output /absolute/fresh/output --ref APPROVED_COMMIT
python3 tools/release.py build --output /absolute/fresh/output
```

`prepare` requires a Git checkout because it selects an exact committed snapshot. The archive itself has no Git history. To rebuild **from an already downloaded archive**, use the following local layout instead:

1. Put the archive in a fresh directory.
2. Create `release.json` from the accompanying release provenance JSON, retaining `source_archive` and `source_sha256`.
3. Run `python3 EXTRACTED_SOURCE/tools/release.py build --output THAT_DIRECTORY`.

The source archive already contains `RELEASE-GRAPHS.json` and all accompanying license files.

The tool extracts another clean copy, creates empty `HOME`/`CARGO_HOME` directories, uses the existing installed Rust toolchains, and runs serial offline/locked builds and the full test gate. It does not download/install a toolchain. Output paths must not contain whitespace. Build logs remain local and are not packaged. The tool refuses to reuse an existing verification directory.

Makefile variables used by the release tool are also available for manual builds:

- `ENGINE_CARGO`, `PLUGIN_CARGO`: exact Cargo/toolchain commands.
- `ENGINE_TARGET_DIR`, `PLUGIN_TARGET_DIR`: separate artifact directories.
- `TARGET`: explicit target triple; normal default is the host.
- `CARGO_FLAGS`: additional Cargo flags such as `--locked --offline`.
- `CARGO_JOBS`: use at most four.

The release tool passes C++ flags on the **make command line**, clears inherited encoded Rust flags/compiler/cache overrides, and sets explicit Rust flags. A plain environment `CXXFLAGS` does not override this Makefile's normal assignment.

## Verification boundaries

The offline gate tests the actual engine and the standalone routing, AU, and CLAP test hosts. VST3 is built, packaged, and inspected but is not thereby tested in a real DAW. Ad-hoc signing verifies local bundle integrity; it is not Developer ID signing, notarization, or Gatekeeper approval. No signing credentials are needed.

Checks cover arm64 architecture, Mach-O deployment target, system-library imports, signatures, and source lock/identity preservation. Notices remain inside `Contents/Resources/licenses` when a bundle is installed; all resource edits precede final signing. Successful builds do not establish old-OS runtime support or reproducible binary bytes across different SDK/compiler versions. No Intel binary is produced.

For first-open warnings on a trusted ad-hoc signed app, use Apple's per-app guidance, not a global Gatekeeper/quarantine bypass. See [first-open guidance](docs/RELEASING.md#first-open-guidance); malware/tampering/damaged warnings are not routine approvals.

Demo MIDI is an optional external download with separate terms; it is never included in binary or source release assets. See `demos/ATTRIBUTION.txt`.
