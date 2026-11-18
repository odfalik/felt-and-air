CXX      := clang++
CXXFLAGS := -std=c++20 -O3 -mcpu=native -fno-math-errno -Wall -Wextra -Wno-unused-parameter -Isrc/core
CORE_SRC := $(wildcard src/core/*.cpp)
CORE_HDR := $(wildcard src/core/*.h)
CORE_OBJ := $(patsubst src/core/%.cpp,build/core/%.o,$(CORE_SRC))
# Same engine with single precision state, for the realtime build and its comparison tools.
FAST     := -DPP_PLATE_REAL=float -DPP_STRING_REAL=float
FAST_OBJ := $(patsubst src/core/%.cpp,build/fast/%.o,$(CORE_SRC))

.PHONY: all test clean app au install-au
all: build/tests build/render build/bench build/report build/metalbench build/render-fast build/bench-fast app

build/core/%.o: src/core/%.cpp $(CORE_HDR)
	@mkdir -p build/core
	$(CXX) $(CXXFLAGS) -c $< -o $@
build/fast/%.o: src/core/%.cpp $(CORE_HDR)
	@mkdir -p build/fast
	$(CXX) $(CXXFLAGS) $(FAST) -c $< -o $@
build/render-fast: src/tools/render.cpp $(FAST_OBJ)
	$(CXX) $(CXXFLAGS) $(FAST) $^ -o $@
build/bench-fast: src/tools/bench.cpp $(FAST_OBJ)
	$(CXX) $(CXXFLAGS) $(FAST) $^ -o $@

build/tests: tests/tests.cpp $(CORE_OBJ)
	$(CXX) $(CXXFLAGS) $^ -o $@
build/render: src/tools/render.cpp $(CORE_OBJ)
	$(CXX) $(CXXFLAGS) $^ -o $@
build/metalbench: src/tools/metalbench.mm
	$(CXX) $(CXXFLAGS) -fobjc-arc -ObjC++ $< -o $@ -framework Foundation -framework Metal
build/autest: src/tools/autest.cpp
	$(CXX) $(CXXFLAGS) $< -o $@ -framework AudioToolbox -framework CoreFoundation
build/report: src/tools/report.cpp $(CORE_OBJ)
	$(CXX) $(CXXFLAGS) $^ -o $@
build/bench: src/tools/bench.cpp $(CORE_OBJ)
	$(CXX) $(CXXFLAGS) $^ -o $@

APP := build/PhysicalPiano.app
app: $(APP)/Contents/MacOS/PhysicalPiano
$(APP)/Contents/MacOS/PhysicalPiano: src/app/main.mm src/app/Info.plist $(CORE_OBJ) $(CORE_HDR)
	@mkdir -p $(APP)/Contents/MacOS
	cp src/app/Info.plist $(APP)/Contents/Info.plist
	$(CXX) $(CXXFLAGS) -fobjc-arc -ObjC++ -DPP_DEMO_DIR='"$(CURDIR)/demos"' src/app/main.mm $(CORE_OBJ) -o $@ \
		-framework AppKit -framework AudioToolbox -framework CoreAudio -framework CoreMIDI
	codesign --force --sign - $(APP) >/dev/null 2>&1 || true

AU := build/PhysicalPiano.component
au: $(AU)/Contents/MacOS/PhysicalPiano
$(AU)/Contents/MacOS/PhysicalPiano: src/au/PhysicalPianoAU.cpp src/au/Info.plist $(CORE_OBJ) $(CORE_HDR)
	@mkdir -p $(AU)/Contents/MacOS
	cp src/au/Info.plist $(AU)/Contents/Info.plist
	$(CXX) $(CXXFLAGS) -bundle src/au/PhysicalPianoAU.cpp $(CORE_OBJ) -o $@ \
		-framework AudioToolbox -framework AudioUnit -framework CoreFoundation
	codesign --force --sign - $(AU) >/dev/null 2>&1 || true
# Copies the plug-in to where hosts look for it, for this user only.
install-au: au
	mkdir -p "$(HOME)/Library/Audio/Plug-Ins/Components"
	rm -rf "$(HOME)/Library/Audio/Plug-Ins/Components/PhysicalPiano.component"
	cp -R $(AU) "$(HOME)/Library/Audio/Plug-Ins/Components/"

test: build/tests
	./build/tests
clean:
	rm -rf build
