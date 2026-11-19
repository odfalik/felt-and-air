# License text provenance

`GPL-3.0.txt` is the unmodified GNU GPL version 3 text downloaded from the official URL:

https://www.gnu.org/licenses/gpl-3.0.txt

SHA-256:

```text
3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986
```

Release preparation verifies this hash and never fetches a replacement. The pinned `vst3-sys` source grants GPLv3-or-later; this full text accompanies VST3 binary distributions. It does not change the MIT license of the original project source or replace third-party/font licenses.

Dependency notices are generated from the exact locked source packages for each selected Cargo graph. `supplemental.json` and the hash-named `upstream/` texts fill publisher archive omissions using exact upstream commits and recorded SHA-256 values; they do not replace the Cargo graph. Both the full source archive and relevant binary assets receive these notices.

Four publisher-declaration exceptions are explicit: `block 0.1.6`, `dispatch 0.2.0`, `malloc_buf 0.0.6`, and source-only `r-efi 6.0.0` provide no separate license text even in the pinned upstream tree. The source/package notices preserve original Cargo declarations/authors and commit/tag/archive provenance. An unmodified standard MIT text from the SPDX license-list-data repository at commit `d46e94e2c78ceede1cfc63cfa0396472d2798d4c` is labeled as a **reference**, not a newly invented copyright grant. `r-efi` retains its exact OR expression with MIT elected from the offered alternatives. `LICENSE-PROVENANCE.json` in generated assets makes these exceptions conspicuous. This is an explicit packaging policy, not legal clearance.

The Oboe wrapper notices retain its Apache declaration, the actual Google Oboe submodule license from commit `86165b8249bc22b9ef70b69e20323244b6f08d88`, and an authoritative standard Apache reference. Font OFL/UFL/Hack/emoji notices remain verbatim, including Bitstream Vera terms. No publisher source or license is relabeled wholesale.

Rust runtime/standard-library notices come from the exact installed toolchains' COPYRIGHT/library reports and license texts, separately from Cargo packages. The compiler/SDK itself is not bundled. See `docs/RELEASING.md`.
