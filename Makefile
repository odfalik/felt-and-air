# The engine is a Rust crate (engine/). This Makefile builds it and the two thin shells
# around it: the Mac app and the Audio Unit. Needs cargo and the Xcode Command Line Tools.
CXX      := clang++
CXXFLAGS := -std=c++20 -O2 -mcpu=native -Wall -Wextra -Wno-unused-parameter -Iengine/include -Isrc/shared
ENGINE   := engine/target/release/libpiano_engine.a
APP      := build/PhysicalPiano.app
AU       := build/PhysicalPiano.component

.PHONY: all engine test app au install-au clean
all: engine app au build/autest build/metalbench

# cargo decides what is out of date.
engine:
	cd engine && cargo build --release
$(ENGINE): engine

test: engine build/autest au
	cd engine && cargo test --release -- --nocapture --test-threads=1
	./build/autest

app: $(APP)/Contents/MacOS/PhysicalPiano
$(APP)/Contents/MacOS/PhysicalPiano: src/app/main.mm src/app/Info.plist src/shared/Engine.h $(ENGINE)
	@mkdir -p $(APP)/Contents/MacOS
	cp src/app/Info.plist $(APP)/Contents/Info.plist
	$(CXX) $(CXXFLAGS) -fobjc-arc -ObjC++ -DPP_DEMO_DIR='"$(CURDIR)/demos"' src/app/main.mm $(ENGINE) -o $@ \
		-framework AppKit -framework AudioToolbox -framework CoreAudio -framework CoreMIDI
	codesign --force --sign - $(APP) >/dev/null 2>&1 || true

au: $(AU)/Contents/MacOS/PhysicalPiano
$(AU)/Contents/MacOS/PhysicalPiano: src/au/PhysicalPianoAU.cpp src/au/Info.plist src/shared/Engine.h $(ENGINE)
	@mkdir -p $(AU)/Contents/MacOS
	cp src/au/Info.plist $(AU)/Contents/Info.plist
	$(CXX) $(CXXFLAGS) -bundle src/au/PhysicalPianoAU.cpp $(ENGINE) -o $@ \
		-framework AudioToolbox -framework AudioUnit -framework CoreFoundation
	codesign --force --sign - $(AU) >/dev/null 2>&1 || true
# Copies the plug-in to where hosts look for it, for this user only.
install-au: au
	mkdir -p "$(HOME)/Library/Audio/Plug-Ins/Components"
	rm -rf "$(HOME)/Library/Audio/Plug-Ins/Components/PhysicalPiano.component"
	cp -R $(AU) "$(HOME)/Library/Audio/Plug-Ins/Components/"

build/autest: src/tools/autest.cpp
	@mkdir -p build
	$(CXX) $(CXXFLAGS) $< -o $@ -framework AudioToolbox -framework CoreFoundation
build/metalbench: src/tools/metalbench.mm
	@mkdir -p build
	$(CXX) $(CXXFLAGS) -fobjc-arc -ObjC++ $< -o $@ -framework Foundation -framework Metal

clean:
	rm -rf build
	cd engine && cargo clean
