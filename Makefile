# Build the Rust engine, shared app/CLAP/VST3 editor, and native Audio Unit shell.
# Needs cargo and the Xcode Command Line Tools.
CARGO_JOBS ?= 4
ENGINE_CARGO ?= cargo
PLUGIN_CARGO ?= cargo
ENGINE_TARGET_DIR ?= engine/target
PLUGIN_TARGET_DIR ?= plugin/target
TARGET ?=
CARGO_FLAGS ?=
CARGO_ARGS = $(if $(TARGET),--target $(TARGET),) $(CARGO_FLAGS)
RELEASE_DIR = $(if $(TARGET),$(TARGET)/,)release
CXX      := clang++
CXXFLAGS := -std=c++20 -O2 -mcpu=native -Wall -Wextra -Wno-unused-parameter -Iengine/include -Isrc/shared
ENGINE   := $(ENGINE_TARGET_DIR)/$(RELEASE_DIR)/libpiano_engine.a
APP      := build/FeltAndAir.app
AU       := build/FeltAndAir.component

.PHONY: all engine test app au clap vst3 install-au install-clap clean
all: engine app au clap build/autest build/claptest build/metalbench

# cargo decides what is out of date.
engine:
	cd engine && CARGO_TARGET_DIR="$(abspath $(ENGINE_TARGET_DIR))" $(ENGINE_CARGO) build -j$(CARGO_JOBS) --release $(CARGO_ARGS)
$(ENGINE): engine

test: engine build/autest build/claptest au clap
	cd engine && CARGO_TARGET_DIR="$(abspath $(ENGINE_TARGET_DIR))" $(ENGINE_CARGO) test -j$(CARGO_JOBS) --release $(CARGO_ARGS) -- --nocapture --test-threads=1
	cd plugin && CARGO_TARGET_DIR="$(abspath $(PLUGIN_TARGET_DIR))" $(PLUGIN_CARGO) test -j$(CARGO_JOBS) --release $(CARGO_ARGS) --features standalone --lib --bins -- --nocapture --test-threads=1
	./build/autest
	./build/claptest

# Same Rust editor as CLAP/VST3. Demos remain external (never bundled or committed).
app:
	cd plugin && CARGO_TARGET_DIR="$(abspath $(PLUGIN_TARGET_DIR))" $(PLUGIN_CARGO) build -j$(CARGO_JOBS) --release $(CARGO_ARGS) --features standalone --bin felt-and-air
	@mkdir -p $(APP)/Contents/MacOS $(APP)/Contents/Resources
	cp src/app/Info.plist $(APP)/Contents/Info.plist
	cp assets/icon/FeltAndAir.icns $(APP)/Contents/Resources/FeltAndAir.icns
	cp $(PLUGIN_TARGET_DIR)/$(RELEASE_DIR)/felt-and-air $(APP)/Contents/MacOS/FeltAndAir
	codesign --force --sign - $(APP)

au: $(AU)/Contents/MacOS/FeltAndAir
$(AU)/Contents/MacOS/FeltAndAir: src/au/PhysicalPianoAU.cpp src/au/Info.plist src/shared/Engine.h $(ENGINE)
	@mkdir -p $(AU)/Contents/MacOS
	cp src/au/Info.plist $(AU)/Contents/Info.plist
	$(CXX) $(CXXFLAGS) -bundle src/au/PhysicalPianoAU.cpp $(ENGINE) -o $@ \
		-framework AudioToolbox -framework AudioUnit -framework CoreFoundation
	codesign --force --sign - $(AU) >/dev/null 2>&1 || true
# Copies the plug-in to where hosts look for it, for this user only.
install-au: au
	mkdir -p "$(HOME)/Library/Audio/Plug-Ins/Components"
	rm -rf "$(HOME)/Library/Audio/Plug-Ins/Components/FeltAndAir.component"
	cp -R $(AU) "$(HOME)/Library/Audio/Plug-Ins/Components/"

# The CLAP plug-in is a second crate (plugin/), on nih-plug, with its own pinned toolchain.
# `make clap VST3=1` would also need the bundle step for a .vst3; note the licence remark in
# plugin/Cargo.toml before distributing one.
CLAP := build/FeltAndAir.clap
clap:
	cd plugin && CARGO_TARGET_DIR="$(abspath $(PLUGIN_TARGET_DIR))" $(PLUGIN_CARGO) build -j$(CARGO_JOBS) --release $(CARGO_ARGS) --lib
	@mkdir -p $(CLAP)/Contents/MacOS
	cp plugin/Info.plist $(CLAP)/Contents/Info.plist
	cp $(PLUGIN_TARGET_DIR)/$(RELEASE_DIR)/libphysical_piano_plugin.dylib $(CLAP)/Contents/MacOS/FeltAndAir
	codesign --force --sign - $(CLAP) >/dev/null 2>&1 || true
# VST3: same crate with the vst3 feature. Its bindings (vst3-sys) are GPLv3, so a binary
# built this way must be distributed under the GPL. Not built by default and not host-tested.
VST3 := build/FeltAndAir.vst3
vst3:
	cd plugin && CARGO_TARGET_DIR="$(abspath $(PLUGIN_TARGET_DIR))" $(PLUGIN_CARGO) build -j$(CARGO_JOBS) --release $(CARGO_ARGS) --features vst3 --lib
	@mkdir -p $(VST3)/Contents/MacOS
	cp plugin/Info.plist $(VST3)/Contents/Info.plist
	plutil -replace CFBundleIdentifier -string com.odfalik.felt-and-air.vst3 $(VST3)/Contents/Info.plist
	printf 'BNDL????' > $(VST3)/Contents/PkgInfo
	cp $(PLUGIN_TARGET_DIR)/$(RELEASE_DIR)/libphysical_piano_plugin.dylib $(VST3)/Contents/MacOS/FeltAndAir
	codesign --force --sign - $(VST3) >/dev/null 2>&1 || true
install-clap: clap
	mkdir -p "$(HOME)/Library/Audio/Plug-Ins/CLAP"
	rm -rf "$(HOME)/Library/Audio/Plug-Ins/CLAP/FeltAndAir.clap"
	cp -R $(CLAP) "$(HOME)/Library/Audio/Plug-Ins/CLAP/"

build/claptest: src/tools/claptest.cpp
	@mkdir -p build
	$(CXX) $(CXXFLAGS) $< -o $@
build/autest: src/tools/autest.cpp
	@mkdir -p build
	$(CXX) $(CXXFLAGS) $< -o $@ -framework AudioToolbox -framework CoreFoundation
build/metalbench: src/tools/metalbench.mm
	@mkdir -p build
	$(CXX) $(CXXFLAGS) -fobjc-arc -ObjC++ $< -o $@ -framework Foundation -framework Metal

clean:
	rm -rf build
	cd engine && cargo clean
	cd plugin && cargo clean
