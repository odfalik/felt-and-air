# Releasing Felt & Air

The local release tool creates experimental **macOS arm64** assets. It never creates a Git tag, contacts GitHub, signs with an identity, installs plug-ins, launches the app, or opens an audio device.

## Release identity and approval

- Human name: **Felt & Air**.
- App/artifact stem: **FeltAndAir**.
- Website slug: **felt-and-air**.
- Repository: `odfalik/physical-piano`.
- Intended first tag: **v0.1.0-preview.1**.

Asset defaults are localized at the top of `tools/release.py` and may be overridden with `--product`/`--version`. They do not stamp or rename source metadata. The requested version must equal the exact plugin package version; all bundle short-version numeric cores and the AU encoded integer version must agree. Bundle build numbers are independent positive integers, consistent across bundles (preview 1 uses `1`), and are recorded rather than stamped. Product sources must already agree with the executable stem. Preparation rejects a mismatch.

Choose a clean, reviewed publication commit whose product identity and runtime paths are ready for distribution. Review outgoing history and author/committer metadata; use a public attribution address rather than exposing a personal mailbox unintentionally. A local tool run does **not** certify Git history, private paths or source contents, and never publishes them. Development smoke-test archives are not release assets.

## Prepare locally (no compilation)

The working tree must be committed and clean. Use a fresh ignored output directory, no whitespace in paths, and sufficient disk: preparation requires at least 4 GiB free; full verification requires at least 6 GiB free and may use more for Cargo targets. Keep at most four Cargo jobs; builds and tests are serial between formats.

```sh
python3 tools/release.py prepare \
  --ref APPROVED_PUBLIC_SNAPSHOT \
  --product FeltAndAir --version 0.1.0-preview.1 \
  --output "$PWD/out/release-preview-1"
```

Preparation uses `git archive`, not a working-tree copy. It includes the exact project source, original lock/manifests, full offline dependency source, GPL text, third-party notices, dependency graphs, build instructions, and normalized immutable build-input provenance. The source archive deliberately has no mutable `source_complete` marker. External `release.json` carries verification state tied to the exact archive SHA-256; the archive is never repacked to flip a status after testing. It then extracts the generated archive and verifies offline Cargo metadata with a fresh empty `CARGO_HOME`, without compiling. `PREPARED.txt` explicitly does **not** claim a verified offline build or complete Corresponding Source.

### Cargo vendor collision

The lock includes `baseview 0.1.0` at two Git revisions. A single `cargo vendor --locked --versioned-dirs --offline` fails on that name/version collision. Do not unify revisions, alter package versions, change the final lock, or silently omit one source.

The bounded accommodation uses two temporary probe manifests: GUI+VST3, and standalone+VST3. Cargo vendors their Git packages and normalizes the package manifests. Every external probe lock identity and checksum must match the original project lock. Git origins get separate Cargo directory-source namespaces; **all** registry lock entries are included from verified cached source files. The final project's Cargo manifest and lock remain byte-for-byte unchanged. Cargo-emitted source descriptors are merged into the plugin's existing config with relative paths. There is no custom resolver.

The private probe records stay outside the source archive. This method still requires a real offline build from a fresh extraction before any source-complete claim. Missing cached dependencies fail offline rather than initiating downloads. There is no toolchain/scanner installation.

## Verify/build (CPU-heavy: coordinate first)

Run one portability build at a time after selecting the publication snapshot:

```sh
python3 tools/release.py build --output "$PWD/out/release-preview-1"
```

The tool extracts the prepared archive into another fresh directory. It uses empty `HOME` and `CARGO_HOME`, the already installed Rust toolchains, offline/locked Cargo, separate per-format target directories, a shared controlled engine target, and at most four jobs. It reuses the Makefile's actual build/bundle/test recipes.

Release choices are explicit:

- `aarch64-apple-darwin`, `apple-m1`, deployment **13.0** for both Rust crates.
- C++ `-arch arm64 -mcpu=apple-m1 -mmacosx-version-min=13.0` passed on the **make command line**.
- Build children receive only a small parent environment allowlist, not credential/token variables, wrappers, encoded Rust flags, target-specific flags, jobservers, or remote caches. Native Cargo build scripts receive explicit Xcode `CC`/`CXX`, M1/minimum-13/path-remapping `CFLAGS`/`CXXFLAGS` too; Makefile C++ flags are also overridden on its command line.
- Checkout, output/temp, Cargo home, Rustup home, and home diagnostic prefixes are remapped to placeholders.
- Rust cdylib `LC_ID_DYLIB` build paths are normalized before final signing.
- The full `make test` gate runs offline, followed by distinct app, CLAP, and VST3 builds. AU is built/tested by that gate.
- Every Mach-O inside each bundle is checked for arm64, macOS platform and numeric deployment version exactly `(13, 0, 0)`, system-only imports, unexpected runtime search paths, and remaining private build/home prefixes, including Cargo/Rustup homes outside `HOME`.
- Format exports are checked: VST3 factory present; CLAP entry present with no VST3 factory; current AU factory present with no obsolete factory. These sanity gates do not certify real-host compatibility.
- The final signed asset tree, resources, install instructions and provenance are byte-scanned for actual private prefixes before ZIP creation; source-owned metadata is also checked. The owner's full native/source audit remains separate.
- Project/runtime/dependency/font notices are inserted **inside** each delivered bundle at `Contents/Resources/licenses` before final signing. They remain present when a user drags only the bundle into place.
- Every Mach-O and final bundle are ad-hoc signed and verified after those resource edits; no signing credentials are accessed.

`VERIFIED.txt` and published provenance are produced only after the clean extracted tests/builds and binary checks pass. A failed attempt remains available for inspection; the tool never deletes or reuses it. Start a new preparation/output directory for another attempt.

## Assets and licenses

Successful verification creates four separate ZIPs (app, AU, CLAP, VST3), a full source archive, provenance, and `SHA256SUMS`. ZIP-level `INSTALL.txt` and source links accompany the bundle; required notices live inside it. Publish the source archive at the **same release download location**, directly beside and linked from the VST3 binary download.

The original engine/application sources are MIT. App/AU/CLAP do not acquire GPL obligations from an unused VST3 feature. They are **not MIT-only distributions**: GUI dependencies include ISC, Apache, Unicode, and separate font terms. All applicable notices accompany each binary.

The pinned `vst3-sys` license grants GPL version 3 **or later**. The VST3 combined binary is offered under GPLv3, with the upstream notice and complete GPLv3 text retained. Its source archive includes all needed project/dependency/build inputs; `git archive` of just this repository or dependency URLs alone are not a substitute. The online download arrangement avoids promising an unfulfilled future written source offer. Do not add a conflicting noncommercial/proprietary EULA.

Bundled fonts retain their actual licenses: SIL OFL, Ubuntu Font License, emoji-font MIT, and Hack's MIT/Bitstream Vera notices. The tool explicitly includes nested font text files; SPDX metadata alone omits relevant detail. Third-party licenses are not relicensed wholesale to GPL or MIT. Notices/source packaging is an engineering compliance measure, not legal assurance about every combined-work question.

Some registry archives omit license files. `licenses/supplemental.json` pins exact upstream commit texts, their URLs and SHA-256 hashes; the generator checks package checksums/VCS identity and keeps publisher Cargo declarations verbatim. Four explicit-declaration exceptions (`block`, `dispatch`, `malloc_buf`, source-only `r-efi`) have no separate upstream text. `LICENSE-PROVENANCE.json` records that fact and the elected MIT option where offered; a labeled unmodified authoritative standard MIT reference is supplied without inventing a copyright holder/year or claiming legal clearance. Unknown or conflicting licensing still fails preparation. The actual pinned Google Oboe submodule notice accompanies its wrapper declarations.

`licenses/GPL-3.0.txt` is the pinned official GNU text; provenance and hash are documented in `licenses/README.md`. Cargo graphs do not enumerate Rust's statically linked standard library/runtime. Source assets include exact installed toolchain COPYRIGHT/library reports and license texts; each binary receives its relevant toolchain notices. These conservative notices do not assert that every listed runtime component is linked. Rust toolchains and Apple SDKs/compiler/system frameworks are build prerequisites, not redistributed compiler/SDK bundles. Exact installed toolchain/SDK versions and public build commands are recorded.

## First-open guidance

Ad-hoc signed builds are not Developer ID signed/notarized. For an unidentified/not-notarized standalone app, Apple's per-app guidance is: attempt to open it, then **System Settings → Privacy & Security → Open Anyway**, **only if the user trusts its source and integrity**. Check the release hashes first. Plugin host approval may differ.

Do not prescribe global Gatekeeper disabling, `spctl` bypasses, or blanket quarantine removal. Do not tell users to blindly override malware, tampering, or damaged-app warnings; those require investigation rather than routine approval. Building from the matching source is an alternative. Apple's guidance: https://support.apple.com/en-us/102445 (published May 27, 2026).

## Publication gate (owner only)

Before publishing source, tags or assets:

1. Audit the final public noreply snapshot, source archive, binary strings, notices, and generated assets for private/company content; do not publish private development history or logs.
2. Confirm names/bundle IDs/host IDs/demo paths and source SHA match that approved snapshot.
3. Review the full offline verification log and artifact hashes. Keep logs local; they can contain absolute paths and are not release assets.
4. Confirm the source download is present and clearly linked next to VST3, and all four ZIPs retain notices.
5. State **experimental**, Apple Silicon only, actual tested macOS version, and the distinction between intended macOS 13 deployment and verified older-OS runtime support. Ad-hoc signing is not notarization/Gatekeeper approval.
6. Report VST3 real-DAW testing separately: compiling/metadata inspection does not establish that compatibility.

No MAESTRO demo media is included. Demo download/license instructions remain separate. Packaging does not authorize playback, microphones, publishing, tagging, or a credential-backed signing action.
