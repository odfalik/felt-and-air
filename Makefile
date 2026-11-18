CXX      := clang++
CXXFLAGS := -std=c++20 -O3 -mcpu=native -fno-math-errno -Wall -Wextra -Wno-unused-parameter -Isrc/core
CORE_SRC := $(wildcard src/core/*.cpp)
CORE_HDR := $(wildcard src/core/*.h)
CORE_OBJ := $(patsubst src/core/%.cpp,build/core/%.o,$(CORE_SRC))
# Same engine with single precision state, for the realtime build and its comparison tools.
FAST     := -DPP_PLATE_REAL=float -DPP_STRING_REAL=float
FAST_OBJ := $(patsubst src/core/%.cpp,build/fast/%.o,$(CORE_SRC))

.PHONY: all test clean render bench
all: build/tests build/render build/bench build/render-fast build/bench-fast

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
build/bench: src/tools/bench.cpp $(CORE_OBJ)
	$(CXX) $(CXXFLAGS) $^ -o $@

test: build/tests
	./build/tests
clean:
	rm -rf build
