#!/usr/bin/env python3
"""Local macOS release preparation/verification. Never publishes, tags, or installs anything."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shlex
import shutil
import stat
import subprocess
import tarfile
import tempfile
import tomllib
import urllib.parse
import zipfile

DEFAULT_PRODUCT = "FeltAndAir"  # Asset identity only; runtime identity belongs to the product sources.
DISPLAY_NAME = "Felt & Air"
DEFAULT_VERSION = "0.1.0-preview.1"
TARGET = "aarch64-apple-darwin"
ENGINE_RUST = "1.96.1"
PLUGIN_RUST = "1.85.0"
GPL_URL = "https://www.gnu.org/licenses/gpl-3.0.txt"
GPL_SHA = "3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986"


def run(args, cwd, env=None, log=None):
    if log:
        with Path(log).open("a") as f:
            f.write("$ " + shlex.join(map(str, args)) + "\n")
            f.flush()
            subprocess.run(args, cwd=cwd, env=env, stdout=f, stderr=subprocess.STDOUT, check=True)
        return ""
    return subprocess.check_output(args, cwd=cwd, env=env, text=True)


def dump(path, data):
    Path(path).write_text(json.dumps(data, indent=2, sort_keys=True) + "\n")


def sha(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as f:
        for b in iter(lambda: f.read(1024 * 1024), b""):
            h.update(b)
    return h.hexdigest()


def cargo(args, cwd, env=None):
    return run(["cargo", f"+{PLUGIN_RUST}", *args], cwd, env)


def packages(lock):
    return tomllib.loads(Path(lock).read_text())["package"]


def identity(p):
    return (p["name"], p["version"], p.get("source"), p.get("checksum"))


def metadata(source, env=None, features="standalone,vst3"):
    args = ["metadata", "--locked", "--offline", "--format-version", "1"]
    if features:
        args += ["--features", features]
    return json.loads(cargo(args, source / "plugin", env))


def selected(source, env, feature):
    # Cargo metadata's resolve edges can include weak optional dependencies that
    # are not built. Use Cargo's actual normal/build tree, then map exact IDs back
    # to metadata for license/source fields (including both same-version Git pins).
    m = metadata(source, env)
    args = ["tree", "--locked", "--offline", "--target", TARGET, "-e", "normal,build",
            "--prefix", "none", "--format", "{p}"]
    if feature:
        args += ["--features", feature]
    found = set()
    for row in cargo(args, source / "plugin", env).splitlines():
        match = re.match(r"(\S+) v([^\s(]+)", row)
        assert match, "unexpected Cargo tree format"
        name, version = match.groups()
        urls = re.findall(r"\((https?://[^)]+)\)", row)
        candidates = [p for p in m["packages"] if p["name"] == name and p["version"] == version]
        if urls:
            url = urllib.parse.unquote(urls[0])
            base, short_commit = url.rsplit("#", 1)
            candidates = [p for p in candidates if p.get("source", "") and p["source"].startswith("git+")
                          and source_key(p["source"])[4:] == base
                          and p["source"].rsplit("#", 1)[1].startswith(short_commit)]
        else:
            candidates = [p for p in candidates if not (p.get("source") or "").startswith("git+")]
        assert len(candidates) == 1, f"ambiguous or missing Cargo identity: {row}"
        if candidates[0].get("source"):
            found.add(candidates[0]["id"])
    return [p for p in m["packages"] if p["id"] in found]


def source_key(url):
    """Cargo source IDs differ only by the final pinned fragment in replacement tables."""
    return urllib.parse.unquote(url.split("#", 1)[0])


def verify_package(root, expected):
    checks = json.loads((root / ".cargo-checksum.json").read_text())
    assert checks["package"] == expected, f"wrong package checksum: {root.name}"
    for relative, digest in checks["files"].items():
        path = root / relative
        assert path.resolve().is_relative_to(root.resolve()) and not path.is_symlink()
        assert sha(path) == digest, f"source file checksum mismatch: {root.name}/{relative}"


def full_vendor(source, work, original_metadata):
    """Cargo cannot vendor two baseview 0.1.0 Git origins into one directory.

    Two temporary probe graphs let Cargo normalize all Git manifests itself. Every
    external probe lock entry must match the ORIGINAL lock, including checksum. Final
    manifest/lock bytes are never edited. ALL registry lock packages are copied, not
    just the probe graphs. Per-origin directory sources disambiguate baseview.
    """
    lock = source / "plugin/Cargo.lock"
    original = packages(lock)
    allowed = {identity(p) for p in original if p.get("source")}
    vendor = source / "vendor"
    registry = vendor / "registry"
    registry.mkdir(parents=True)
    by_id = {(p["name"], p["version"], p.get("source")): p for p in original_metadata["packages"]}
    checked_git = set()
    for package in original_metadata["packages"]:
        origin = package.get("source", "") or ""
        if not origin.startswith("git+") or origin in checked_git:
            continue
        checkout = Path(package["manifest_path"]).parent
        while not (checkout / ".git").exists():
            assert checkout != checkout.parent, "missing Cargo Git checkout"
            checkout = checkout.parent
        assert run(["git", "rev-parse", "HEAD"], checkout).strip() == origin.rsplit("#", 1)[1]
        assert not run(["git", "status", "--porcelain", "--untracked-files=no"], checkout).strip(), "modified Git dependency checkout"
        checked_git.add(origin)
    for p in original:
        if not p.get("source", "").startswith("registry+"):
            continue
        assert p["source"] == "registry+https://github.com/rust-lang/crates.io-index", "unexpected registry"
        cached = Path(by_id[(p["name"], p["version"], p["source"])]["manifest_path"]).parent
        # Registry caches contain canonical .crate archives, not vendor checksum files.
        # Check the archive against the ORIGINAL lock, then generate Cargo's directory-
        # source checksum manifest from that exact package (never from mutable src/).
        crate = cached.parents[2] / "cache" / cached.parent.name / (cached.name + ".crate")
        assert sha(crate) == p["checksum"], f"registry archive checksum mismatch: {cached.name}"
        with tarfile.open(crate) as t:
            assert all(Path(f.name).parts[0] == cached.name for f in t.getmembers())
            t.extractall(registry, filter="data")
        dest = registry / cached.name
        dump(dest / ".cargo-checksum.json", {"package": p["checksum"],
             "files": {str(f.relative_to(dest)): sha(f) for f in sorted(dest.rglob("*")) if f.is_file()}})
        verify_package(dest, p["checksum"])
    plugin_toml = tomllib.loads((source / "plugin/Cargo.toml").read_text())
    nih = plugin_toml["dependencies"]["nih_plug"]
    egui = plugin_toml["dependencies"]["nih_plug_egui"]
    replacements = {"crates-io": {"replace-with": "release-registry"},
                    "release-registry": {"directory": "../vendor/registry"}}
    copied = set()
    for kind in ["gui", "standalone"]:
        probe = work / kind
        (probe / "src").mkdir(parents=True)
        (probe / "src/lib.rs").write_text("")
        text = '[package]\nname="release_probe"\nversion="0.0.0"\nedition="2021"\n[dependencies]\n'
        features = '["vst3"]' if kind == "gui" else '["standalone","vst3"]'
        text += f'nih_plug={{git={json.dumps(nih["git"])},rev={json.dumps(nih["rev"])},default-features=false,features={features}}}\n'
        if kind == "gui":
            text += f'nih_plug_egui={{git={json.dumps(egui["git"])},rev={json.dumps(egui["rev"])}}}\n'
        (probe / "Cargo.toml").write_text(text)
        shutil.copyfile(lock, probe / "Cargo.lock")
        # Probe locks may prune unused packages, but may NOT change any external identity.
        config = cargo(["vendor", "--versioned-dirs", "--offline", "vendor"], probe)
        probe_packages = packages(probe / "Cargo.lock")
        assert all(identity(p) in allowed for p in probe_packages if p.get("source")), "probe changed a locked dependency"
        tables = tomllib.loads(config)["source"]
        for p in probe_packages:
            origin = p.get("source", "")
            if not origin.startswith("git+"):
                continue
            name = "git-" + hashlib.sha256(origin.encode()).hexdigest()[:16]
            folder = vendor / name
            folder.mkdir(exist_ok=True)
            key = source_key(origin)
            desc = {k: v for k, v in tables[key].items() if k != "replace-with"}
            replacements[key] = {**desc, "replace-with": name}
            replacements[name] = {"directory": "../vendor/" + name}
            src = probe / "vendor" / (p["name"] + "-" + p["version"])
            dst = folder / src.name
            if not dst.exists():
                verify_package(src, None)
                shutil.copytree(src, dst)
            copied.add(identity(p))
    assert copied == {identity(p) for p in original if p.get("source", "").startswith("git+")}, "missing Git dependency"
    config = source / "plugin/.cargo/config.toml"
    assert "[source." not in config.read_text(), "source config already exists; review before merging"
    with config.open("a") as f:
        f.write("\n# Offline release sources; original dependency identities/lock retained.\n")
        for key, fields in sorted(replacements.items()):
            f.write(f"\n[source.{json.dumps(key)}]\n")
            for k, v in fields.items():
                f.write(f"{k} = {json.dumps(v)}\n")
    return {"registry_packages": sum(p.get("source", "").startswith("registry+") for p in original),
            "git_packages": len(copied), "method": "Cargo-generated split Git sources + all locked registry sources"}


def legal_files(root):
    return [p for p in sorted(root.rglob("*")) if p.is_file()
            and p.suffix.lower() not in {".rs", ".py", ".toml", ".json"}
            and (any(s in p.name.lower() for s in ("license", "copying", "copyright", "notice"))
                 or p.name.lower() in {"ofl.txt", "ufl.txt", "hack-regular.txt"})]


def notices(pkgs, destination, supplement_root):
    destination.mkdir(parents=True)
    supplements = json.loads((supplement_root / "supplemental.json").read_text())
    index = []
    for p in sorted(pkgs, key=lambda p: p["id"]):
        assert p.get("license"), f"missing publisher license declaration: {p['id']}"
        root = Path(p["manifest_path"]).parent
        files = [(f, f.relative_to(root)) for f in legal_files(root)]
        # Git workspace subcrates often inherit a LICENSE from a parent (COM is MIT,
        # not the GPL license at the vst3-sys root). Keep the closest ancestor's notice.
        if p["source"].startswith("git+") and not files:
            parent = root
            while parent != parent.parent:
                inherited = [f for f in parent.iterdir() if f.is_file() and
                             any(s in f.name.lower() for s in ("license", "copying", "copyright", "notice"))]
                if inherited:
                    files = [(f, Path(f.name)) for f in inherited]
                    break
                if (parent / ".git").exists():
                    break
                parent = parent.parent
        extra = supplements.get(p["name"] + "@" + p["version"])
        if extra:
            assert p["source"].startswith("registry+")
            assert json.loads((root / ".cargo-checksum.json").read_text())["package"] == extra["registry_checksum"]
            vcs = root / ".cargo_vcs_info.json"
            if vcs.exists():
                assert json.loads(vcs.read_text())["git"]["sha1"] == extra["source_commit"]
            else:
                assert extra.get("version_tag"), "missing exact version-tag provenance"
            if extra.get("metadata_declaration_fallback"):
                assert extra["publisher_declared_license"] == p["license"]
                assert extra["elected_license"] in p["license"].split(" OR ")
            for f in extra["files"]:
                text = supplement_root / f["path"]
                assert sha(text) == f["sha256"], "changed pinned supplemental license"
                files.append((text, Path("UPSTREAM") / text.name))
            # Verbatim publisher declarations, including authors. Never invent a
            # copyright holder/year to fill a missing upstream license notice.
            for filename in ["Cargo.toml", "Cargo.toml.orig", ".cargo_vcs_info.json"]:
                if (root / filename).exists():
                    files.append((root / filename, Path("PUBLISHER-" + filename.lstrip("."))))
        assert files, f"no license/notice files found: {p['id']}"
        key = p["name"] + "-" + p["version"] + "-" + hashlib.sha256(p["source"].encode()).hexdigest()[:8]
        for f, relative in files:
            dest = destination / key / relative
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(f, dest)
        index.append({"name": p["name"], "version": p["version"], "source": p["source"],
                      "license_expression": p.get("license"),
                      "license_files": [str(Path(key) / r) for _, r in files],
                      "supplemental_provenance": extra})
    dump(destination / "INDEX.json", index)
    (destination / "README.txt").write_text(
        "Third-party source notices, retained verbatim. INDEX.json identifies exact packages.\n"
        "These components/fonts retain their own licenses; this is not an MIT-only distribution.\n"
        "License expressions are Cargo metadata, not a substitute for the accompanying texts.\n")


def rust_runtime_notices(source):
    records = []
    for version in [ENGINE_RUST, PLUGIN_RUST]:
        sysroot = Path(run(["rustc", f"+{version}", "--print", "sysroot"], source).strip())
        docs = sysroot / "share/doc/rust"
        # New toolchains provide a focused standard-library report; older ones
        # provide COPYRIGHT plus the dual-license texts. No compiler/SDK is copied.
        if (docs / "COPYRIGHT-library.html").exists():
            files = [docs / "COPYRIGHT-library.html", *sorted((docs / "licenses").glob("*"))]
        else:
            files = [docs / name for name in ["COPYRIGHT", "LICENSE-MIT", "LICENSE-APACHE"]]
        copied = {}
        for f in files:
            assert f.is_file(), f"install the exact toolchain notice files explicitly: Rust {version}"
            relative = f.relative_to(docs)
            dest = source / "RUST-RUNTIME-NOTICES" / version / relative
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(f, dest)
            copied[str(relative)] = sha(dest)
        records.append({"toolchain": version, "origin": "installed official Rust toolchain share/doc/rust",
                        "files_sha256": copied})
    return records


def archive(source, output):
    with output.open("wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode="w") as tar:
            for p in [source, *sorted(source.rglob("*"))]:
                rel = p.relative_to(source.parent)
                parts = p.relative_to(source).parts
                assert ".git" not in parts
                if not parts or parts[0] != "vendor":
                    assert not {"target", "out"}.intersection(parts)
                    assert p.suffix.lower() not in {".mid", ".midi", ".wav", ".mp3"}, f"demo/media in source: {rel}"
                info = tar.gettarinfo(str(p), str(rel))
                info.uid = info.gid = info.mtime = 0
                info.uname = info.gname = ""
                if p.is_file():
                    with p.open("rb") as f:
                        tar.addfile(info, f)
                else:
                    tar.addfile(info)


def path_variants(path):
    path = Path(path).expanduser()
    return list(dict.fromkeys([path, path.resolve()]))


def private_paths(root, output, env=None):
    home = Path.home()
    paths = [root, output, home, os.environ.get("CARGO_HOME", home / ".cargo"),
             os.environ.get("RUSTUP_HOME", home / ".rustup"), tempfile.gettempdir()]
    if env:
        paths += [env[key] for key in ["HOME", "CARGO_HOME", "RUSTUP_HOME", "SDKROOT"]]
    return list(dict.fromkeys(p for path in paths for p in path_variants(path)))


def privacy_scan(folder, prefixes, excluded=()):
    needles = [str(p).encode() for p in prefixes]
    for f in folder.rglob("*"):
        if not f.is_file() or any(p in excluded for p in f.relative_to(folder).parts):
            continue
        data = f.read_bytes()
        assert not any(n in data for n in needles), f"private path remains in asset: {f.relative_to(folder)}"


def isolated_env(output, source):
    original_home = Path.home().resolve()
    cargo_home = Path(os.environ.get("CARGO_HOME", original_home / ".cargo")).resolve()
    rustup_home = Path(os.environ.get("RUSTUP_HOME", original_home / ".rustup")).resolve()
    # Minimal child environment: no credential/token variables, compiler wrappers,
    # encoded Rust flags, target-specific native flags, jobservers or remote caches.
    keep = {"PATH", "LANG", "LC_ALL", "LC_CTYPE", "TMPDIR", "TMP", "TEMP", "DEVELOPER_DIR"}
    env = {key: value for key, value in os.environ.items() if key in keep}
    home = output / "isolated-home"
    empty_cargo = output / "isolated-cargo"
    home.mkdir()
    empty_cargo.mkdir()
    env.update(HOME=str(home), CARGO_HOME=str(empty_cargo), RUSTUP_HOME=str(rustup_home),
               CARGO_NET_OFFLINE="true", CARGO_BUILD_JOBS="4", MACOSX_DEPLOYMENT_TARGET="13.0")
    env["SDKROOT"] = run(["xcrun", "--sdk", "macosx", "--show-sdk-path"], source, env).strip()
    assert not any(c.isspace() for c in env["SDKROOT"]), "SDK path must not contain whitespace"
    # Broad prefixes first: Rust applies the last matching remap. No encoded flags may win.
    originals = [(Path.home(), "/home"), (os.environ.get("CARGO_HOME", cargo_home), "/cargo-home"),
                 (os.environ.get("RUSTUP_HOME", rustup_home), "/rustup"),
                 (tempfile.gettempdir(), "/tmp"), (Path(__file__).resolve().parents[1], "/checkout"),
                 (output, "/release"), (env["SDKROOT"], "/sdk"),
                 (home, "/home"), (empty_cargo, "/cargo-home"), (source, "/src")]
    mappings = [(p, replacement) for path, replacement in originals for p in path_variants(path)]
    flags = ["-C", "target-cpu=apple-m1"] + [f"--remap-path-prefix={a}={b}" for a, b in mappings]
    env["RUSTFLAGS"] = shlex.join(flags)
    # Cargo splits RUSTFLAGS on whitespace (not shell quoting); fail rather than mis-remap.
    assert all(not any(c.isspace() for c in str(a)) for a, _ in mappings), "release paths must not contain whitespace"
    env["RUSTFLAGS"] = " ".join(flags)
    native_flags = ["-O2", "-arch", "arm64", "-mcpu=apple-m1", "-mmacosx-version-min=13.0",
                    "-isysroot", env["SDKROOT"], *[f"-ffile-prefix-map={a}={b}" for a, b in mappings]]
    env["CC"] = run(["xcrun", "--sdk", "macosx", "--find", "clang"], source, env).strip()
    env["CXX"] = run(["xcrun", "--sdk", "macosx", "--find", "clang++"], source, env).strip()
    env["RUSTFLAGS"] += " -C linker=" + env["CC"] + " -C link-arg=-mmacosx-version-min=13.0"
    env["AR"] = run(["xcrun", "--sdk", "macosx", "--find", "ar"], source, env).strip()
    env["RANLIB"] = run(["xcrun", "--sdk", "macosx", "--find", "ranlib"], source, env).strip()
    env["CFLAGS"] = env["CXXFLAGS"] = " ".join(native_flags)
    cxx = ["-std=c++20", "-Wall", "-Wextra", "-Wno-unused-parameter", "-Iengine/include", "-Isrc/shared", *native_flags]
    return env, " ".join(cxx)


def macos_version(value):
    assert re.fullmatch(r"\d+(?:\.\d+){0,2}", value), "invalid numeric macOS version"
    parts = tuple(map(int, value.split(".")))
    return parts + (0,) * (3 - len(parts))


def check_bundle_version(plist, version):
    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", version)
    assert match, "release version must be semantic major.minor.patch[-prerelease]"
    core = ".".join(match.groups())
    assert plist["CFBundleShortVersionString"] == core, "bundle short version differs from plugin numeric core"
    build_version = plist["CFBundleVersion"]
    assert re.fullmatch(r"[1-9]\d*", build_version), "bundle build number must be a positive integer"
    major, minor, patch = map(int, match.groups())
    assert major < 256 and minor < 256 and patch < 256, "version exceeds AU integer component range"
    for component in plist.get("AudioComponents", []):
        assert component["version"] == (major << 16 | minor << 8 | patch), "AU component integer version differs from plugin numeric core"
    return build_version


def prepare(args, root, output):
    assert not run(["git", "status", "--porcelain"], root).strip(), "commit changes before preparing exact source"
    commit = run(["git", "rev-parse", args.ref + "^{commit}"], root).strip()
    version = args.version
    name = f"{args.product}-{version}"
    assert re.fullmatch(r"[A-Za-z0-9_.-]+", name), "use an ASCII asset/product slug"
    source = output / name
    source.mkdir()
    with tempfile.TemporaryFile() as f:
        subprocess.run(["git", "archive", commit], cwd=root, stdout=f, check=True)
        f.seek(0)
        with tarfile.open(fileobj=f) as t:
            t.extractall(source, filter="data")
    assert tomllib.loads((source / "plugin/Cargo.toml").read_text())["package"]["version"] == version, "asset version differs from the exact plugin package version"
    bundle_numbers = {check_bundle_version(plistlib.loads((source / plist).read_bytes()), version)
                      for plist in ["src/app/Info.plist", "src/au/Info.plist", "plugin/Info.plist"]}
    assert len(bundle_numbers) == 1, "bundle build numbers disagree"
    assert plistlib.loads((source / "src/app/Info.plist").read_bytes())["CFBundleExecutable"] == args.product, "product stem must match the source identity; --product does not rename code"
    assert sha(source / "licenses/GPL-3.0.txt") == GPL_SHA
    before = {p: sha(source / "plugin" / p) for p in ["Cargo.toml", "Cargo.lock"]}
    m = metadata(source)
    vendor = full_vendor(source, output / "private-probes", m)
    external = [p for p in m["packages"] if p.get("source")]
    legal_sources = [dict(p) for p in external]
    for package in legal_sources:
        if package["source"].startswith("registry+"):
            package["manifest_path"] = str(source / "vendor/registry" / (package["name"] + "-" + package["version"]) / "Cargo.toml")
    notices(legal_sources, source / "THIRD_PARTY", source / "licenses")
    notice_index = json.loads((source / "THIRD_PARTY/INDEX.json").read_text())
    dump(source / "LICENSE-PROVENANCE.json", [n for n in notice_index
         if (n.get("supplemental_provenance") or {}).get("metadata_declaration_fallback")])
    runtime = rust_runtime_notices(source)
    graphs = {}
    for format_, feature in [("app", "standalone"), ("clap", ""), ("vst3", "vst3")]:
        graph = selected(source, None, feature)
        ids = {(p["name"], p["version"], p["source"]) for p in graph}
        graphs[format_] = [n for n in notice_index if (n["name"], n["version"], n["source"]) in ids]
    info = {"commit": commit, "product": args.product, "display_name": DISPLAY_NAME,
            "version": version, "bundle_build_number": next(iter(bundle_numbers)), "source_root": name,
            "target": TARGET, "target_cpu": "apple-m1", "deployment_target": "13.0", "max_jobs": 4,
            "engine_toolchain": ENGINE_RUST, "plugin_toolchain": PLUGIN_RUST,
            "engine_rustc": run(["rustc", f"+{ENGINE_RUST}", "--version"], root).strip(),
            "plugin_rustc": run(["rustc", f"+{PLUGIN_RUST}", "--version"], root).strip(),
            "xcode": run(["xcodebuild", "-version"], root).strip(),
            "clang": run(["xcrun", "clang++", "--version"], root).splitlines()[0],
            "sdk": run(["xcrun", "--sdk", "macosx", "--show-sdk-version"], root).strip(),
            "gpl_text": {"url": GPL_URL, "sha256": GPL_SHA}, "vendor": vendor,
            "rust_runtime_notices": runtime,
            "publication_audit": "External owner prerequisite; not certified by this local tool",
            "source_complete": False}
    # Immutable source inputs only. Mutable verification state belongs to external
    # release.json, tied to this archive's hash; never repack after a successful test.
    dump(source / "RELEASE-PROVENANCE.json", {k: v for k, v in info.items() if k != "source_complete"})
    dump(source / "RELEASE-GRAPHS.json", graphs)
    assert all(sha(source / "plugin" / p) == h for p, h in before.items()), "final manifest/lock changed"
    privacy_scan(source, private_paths(root, output), excluded={"vendor"})
    packed = output / (name + "-source.tar.gz")
    archive(source, packed)
    info.update(source_archive=packed.name, source_sha256=sha(packed))
    dump(output / "release.json", info)
    # Fresh CARGO_HOME with no registry index/git checkouts proves the source replacements.
    check = output / "metadata-check"
    check.mkdir()
    extracted = check / name
    with tarfile.open(packed) as t:
        t.extractall(check, filter="data")
    env, _ = isolated_env(check, extracted)
    actual = metadata(extracted, env)
    original_ids = {(p["name"], p["version"], p.get("source")) for p in external}
    assert original_ids == {(p["name"], p["version"], p.get("source")) for p in actual["packages"] if p.get("source")}
    for n in before:
        assert sha(extracted / "plugin" / n) == before[n]
    (output / "PREPARED.txt").write_text("Extracted offline metadata verified. NOT yet an offline build or source-complete claim.\n")
    print(f"Prepared {packed.name}; extracted offline metadata passed. Build verification NOT RUN.")


def audit_bundle(bundle, env, log, private_prefixes, version, build_number):
    plist = plistlib.loads((bundle / "Contents/Info.plist").read_bytes())
    assert check_bundle_version(plist, version) == build_number, "delivered build number differs from source"
    if bundle.suffix == ".app" or "LSMinimumSystemVersion" in plist:
        assert macos_version(plist["LSMinimumSystemVersion"]) == (13, 0, 0), "bundle declares a different OS floor"
    main = bundle / "Contents/MacOS" / plist["CFBundleExecutable"]
    magics = {bytes.fromhex(x) for x in ["cffaedfe", "feedfacf", "cefaedfe", "feedface", "cafebabe", "bebafeca", "cafebabf", "bfbafeca"]}
    binaries = []
    for f in sorted(bundle.rglob("*")):
        if f.is_file():
            with f.open("rb") as stream:
                if stream.read(4) in magics:
                    binaries.append(f)
    assert main in binaries, "missing bundle executable"
    result = []
    for binary in binaries:
        assert run(["lipo", "-archs", str(binary)], bundle.parent, env).strip() == "arm64"
        # Rust cdylibs normally contain the PRIVATE build path as LC_ID_DYLIB.
        # It is not an import: normalize this ID before the final ad-hoc signature.
        ids = run(["otool", "-D", str(binary)], bundle.parent, env).splitlines()[1:]
        if ids:
            public_id = "@rpath/" + bundle.name + "/" + str(binary.relative_to(bundle))
            run(["install_name_tool", "-id", public_id, str(binary)], bundle.parent, env, log)
        commands = run(["otool", "-l", str(binary)], bundle.parent, env)
        versions = re.findall(r"cmd LC_BUILD_VERSION\n\s*cmdsize \d+\n\s*platform (\d+)\n\s*minos ([0-9.]+)", commands)
        assert len(versions) == 1 and versions[0][0] == "1", "expected one macOS LC_BUILD_VERSION"
        assert macos_version(versions[0][1]) == (13, 0, 0), "wrong Mach-O deployment target"
        imports = run(["otool", "-L", str(binary)], bundle.parent, env).splitlines()[1:]
        if ids:
            imports = imports[1:]  # LC_ID_DYLIB is printed first, but is not a dependency.
        for line in imports:
            library = line.strip().split(" (", 1)[0]
            assert library.startswith(("/usr/lib/", "/System/Library/")), f"unexpected non-system import: {library}"
        for value in re.findall(r"cmd LC_RPATH\n.*?\n\s*path (.+?) \(offset", commands):
            assert value.startswith(("/usr/lib/", "/System/Library/")), "unexpected runtime search path"
        data = binary.read_bytes()
        assert not any(str(p).encode() in data for p in private_prefixes), "private build/home path remains in binary"
        result.append({"path": str(binary.relative_to(bundle)), "architecture": "arm64", "minos": "13.0",
                       "imports": [x.strip() for x in imports]})
    symbols = {line.split()[-1].removeprefix("_") for line in run(["nm", "-gU", str(main)], bundle.parent, env).splitlines() if line.strip()}
    if bundle.suffix == ".vst3":
        assert "GetPluginFactory" in symbols, "missing VST3 factory export"
    else:
        assert "GetPluginFactory" not in symbols, "GPL/VST3 feature contamination in non-VST3 asset"
    if bundle.suffix == ".clap":
        assert "clap_entry" in symbols, "missing CLAP entry"
    if bundle.suffix == ".component":
        assert plist["CFBundleExecutable"] + "Factory" in symbols, "missing AU factory"
        assert "PhysicalPianoFactory" not in symbols, "obsolete AU factory exported"
    for binary in binaries:
        run(["codesign", "--force", "--timestamp=none", "--sign", "-", str(binary)], bundle.parent, env, log)
    run(["codesign", "--force", "--timestamp=none", "--sign", "-", str(bundle)], bundle.parent, env, log)
    run(["codesign", "--verify", "--deep", "--strict", "--verbose=2", str(bundle)], bundle.parent, env, log)
    for binary in binaries:
        run(["codesign", "--verify", "--strict", str(binary)], bundle.parent, env, log)
    for record, binary in zip(result, binaries):
        record["sha256"] = sha(binary)
    return {"mach_o": result, "plist_minimum": plist.get("LSMinimumSystemVersion"),
            "format_exports_checked": True, "signature": "ad-hoc; verified", "older_os_runtime_test": "NOT PERFORMED"}


def zip_asset(folder, path):
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as z:
        for f in sorted(folder.rglob("*")):
            if not f.is_file():
                continue
            assert not f.is_symlink(), "review bundle symlinks before packaging"
            i = zipfile.ZipInfo(str(f.relative_to(folder)), date_time=(1980, 1, 1, 0, 0, 0))
            i.external_attr = (stat.S_IMODE(f.stat().st_mode) | stat.S_IFREG) << 16
            i.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(i, f.read_bytes())


def install_instructions(info, format_):
    destinations = {"app": "/Applications", "au": "~/Library/Audio/Plug-Ins/Components",
                    "clap": "~/Library/Audio/Plug-Ins/CLAP", "vst3": "~/Library/Audio/Plug-Ins/VST3"}
    return (f"{DISPLAY_NAME} {info['version']} — experimental macOS arm64 {format_}\n\n"
            f"Copy the bundle to {destinations[format_]}. Nothing is installed automatically.\n"
            "Check SHA256SUMS and trust the source before opening. This build is ad-hoc\n"
            "signed, not Developer ID signed/notarized. After attempting to open a trusted\n"
            "app, Apple's per-app guidance is System Settings > Privacy & Security >\n"
            "Open Anyway. Only use that approval for the unidentified/not-notarized warning\n"
            "if you trust its source and integrity; never blindly override malware,\n"
            "tampering, or damaged-app warnings. Plugin host approval may differ.\n"
            "Do not disable Gatekeeper globally or remove quarantine as a blanket fix.\n"
            "Apple guidance: https://support.apple.com/en-us/102445\n\n"
            f"Source build alternative: {info['source_archive']}, BUILDING.md.\n"
            "The deployment target is macOS13.0; actual test OS is in release provenance.\n"
            "This is not certification of older-macOS or real-DAW compatibility.\n")


def build(root, output):
    assert platform.system() == "Darwin" and platform.machine() == "arm64", "macOS arm64 host required"
    info = json.loads((output / "release.json").read_text())
    packed = output / info["source_archive"]
    assert sha(packed) == info["source_sha256"]
    work = output / "build-check"
    work.mkdir()  # Never reuse old targets or bundles, or silently delete a prior attempt.
    with tarfile.open(packed) as t:
        t.extractall(work, filter="data")
    source = work / info["source_root"]
    env, cxx = isolated_env(work, source)
    targets = work / "targets"
    common = ["CARGO_JOBS=4", f"TARGET={TARGET}", "CARGO_FLAGS=--locked --offline",
              "CXX=" + run(["xcrun", "--find", "clang++"], source, env).strip(),
              f"ENGINE_CARGO=cargo +{ENGINE_RUST}", f"PLUGIN_CARGO=cargo +{PLUGIN_RUST}",
              f"ENGINE_TARGET_DIR={targets / 'engine'}", f"CXXFLAGS={cxx}"]
    assert info["xcode"] == run(["xcodebuild", "-version"], source, env).strip(), "Xcode changed since preparation"
    assert info["clang"] == run(["xcrun", "clang++", "--version"], source, env).splitlines()[0], "compiler changed since preparation"
    assert info["sdk"] == run(["xcrun", "--sdk", "macosx", "--show-sdk-version"], source, env).strip(), "SDK changed since preparation"
    log = work / "private-build.log"
    run(["make", "-j1", "test", *common, f"PLUGIN_TARGET_DIR={targets / 'tests'}"], source, env, log)
    for format_ in ["app", "clap", "vst3"]:
        run(["make", "-j1", format_, *common, f"PLUGIN_TARGET_DIR={targets / format_}"], source, env, log)
    # AU was built and tested by make test using the same controlled engine/C++ flags.
    info["tested_macos"] = run(["sw_vers", "-productVersion"], source, env).strip()
    info["path_remaps"] = {"checkout": "/src", "output": "/release", "HOME": "/home", "CARGO_HOME": "/cargo-home", "RUSTUP_HOME": "/rustup", "SDKROOT": "/sdk", "temporary": "/tmp"}
    prefixes = private_paths(root, output, env)
    info["source_complete"] = True  # Only after the actual clean extracted offline tests/builds.
    graphs = json.loads((source / "RELEASE-GRAPHS.json").read_text())
    assets = output / "assets"
    assets.mkdir()
    prefix = info["product"] + "-" + info["version"]
    for format_, suffix in [("app", ".app"), ("au", ".component"), ("clap", ".clap"), ("vst3", ".vst3")]:
        bundle = source / "build" / (info["product"] + suffix)
        folder = work / (format_ + "-asset")
        folder.mkdir()
        delivered = folder / bundle.name
        shutil.copytree(bundle, delivered)
        resources = delivered / "Contents/Resources/licenses"
        resources.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source / "LICENSE", resources / "LICENSE-MIT.txt")
        source_notices = source / "THIRD_PARTY"
        chosen = graphs[format_] if format_ != "au" else []
        legal = resources / "THIRD_PARTY"
        legal.mkdir()
        for p in chosen:
            for relative in p["license_files"]:
                dest = legal / relative
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source_notices / relative, dest)
        dump(legal / "INDEX.json", chosen)
        dump(resources / "LICENSE-PROVENANCE.json", [n for n in chosen
             if (n.get("supplemental_provenance") or {}).get("metadata_declaration_fallback")])
        runtime_version = ENGINE_RUST if format_ == "au" else PLUGIN_RUST
        shutil.copytree(source / "RUST-RUNTIME-NOTICES" / runtime_version,
                        resources / "RUST-RUNTIME-NOTICES" / runtime_version)
        notice = "Original Felt & Air sources are MIT; bundled dependencies/fonts retain their own licenses.\n"
        if format_ == "vst3":
            shutil.copyfile(source / "licenses/GPL-3.0.txt", resources / "COPYING-GPL-3.0.txt")
            notice += "This combined VST3 binary is distributed under GNU GPLv3; pinned vst3-sys grants v3-or-later.\n"
        notice += f"Matching corresponding source: {info['source_archive']} (same release download location).\n"
        notice += "LICENSE-PROVENANCE.json records declared-license packages without a separate publisher text; standard reference texts are not fabricated publisher notices.\n"
        notice += "RUST-RUNTIME-NOTICES retains relevant toolchain runtime attribution outside the Cargo dependency graph.\n"
        notice += "Experimental, ad-hoc signed, not notarized. Deployment floor is not an older-OS runtime certification.\n"
        (resources / "NOTICE.txt").write_text(notice)
        bundle_info = {**info, "format": format_, "rust_runtime_notices":
                       [r for r in info["rust_runtime_notices"] if r["toolchain"] == runtime_version]}
        dump(resources / "BUILD-PROVENANCE.json", bundle_info)
        # No resource edits after this point: notices must survive dragging the bundle
        # into place and be sealed by the final signature, not left only beside it.
        audit = audit_bundle(delivered, env, log, prefixes, info["version"], info["bundle_build_number"])
        (folder / "NOTICE.txt").write_text(notice + "License resources are inside the bundle: Contents/Resources/licenses.\n")
        (folder / "INSTALL.txt").write_text(install_instructions(info, format_))
        dump(folder / "RELEASE-PROVENANCE.json", {**bundle_info, "binary_audit": audit})
        privacy_scan(folder, prefixes)  # Final signed bytes, resources, INSTALL, provenance.
        zip_asset(folder, assets / f"{prefix}-macos-arm64-{format_}{'-GPLv3' if format_ == 'vst3' else ''}.zip")
    shutil.copyfile(packed, assets / packed.name)
    dump(assets / "RELEASE-PROVENANCE.json", info)
    privacy_scan(assets, prefixes, excluded={p.name for p in assets.iterdir() if p.suffix in {".zip", ".gz"}})
    (assets / "SHA256SUMS").write_text("".join(f"{sha(p)}  {p.name}\n" for p in sorted(assets.iterdir()) if p.is_file()))
    (output / "VERIFIED.txt").write_text("Clean extracted offline build/tests + metadata/signature checks passed. Publication review still required.\n")
    print(f"Verified local assets in {assets}. Nothing published/tagged/installed.")


def require_toolchains(root):
    installed = run(["rustup", "toolchain", "list"], root)
    for version in [ENGINE_RUST, PLUGIN_RUST]:
        assert any(line.startswith(version + "-") for line in installed.splitlines()), f"install Rust {version} explicitly before running; this tool never installs toolchains"


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("stage", choices=["prepare", "build"])
    p.add_argument("--output", type=Path, required=True, help="fresh local output directory, never committed")
    p.add_argument("--ref", default="HEAD", help="exact approved publication snapshot (prepare only)")
    p.add_argument("--product", default=DEFAULT_PRODUCT, help="asset/bundle stem; must match source product identity")
    p.add_argument("--version", default=DEFAULT_VERSION, help="asset version (not a tag-creation command)")
    args = p.parse_args()
    if not __debug__:
        raise SystemExit("Release guards require Python assertions; do not use python -O")
    root = Path(__file__).resolve().parents[1]
    require_toolchains(root)
    output = args.output.expanduser().resolve()
    if args.stage == "prepare":
        output.mkdir(parents=True, exist_ok=False)
    assert shutil.disk_usage(output).free >= (4 if args.stage == "prepare" else 6) * 1024**3, "insufficient free disk space"
    if args.stage == "prepare":
        prepare(args, root, output)
    else:
        build(root, output)


if __name__ == "__main__":
    main()
