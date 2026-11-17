CXX      := clang++
CXXFLAGS := -std=c++20 -O3 -mcpu=native -fno-math-errno -Wall -Wextra -Wno-unused-parameter -Isrc/core
CORE_SRC := $(wildcard src/core/*.cpp)
CORE_OBJ := $(patsubst src/core/%.cpp,build/core/%.o,$(CORE_SRC))

.PHONY: all test clean render bench
all: build/tests build/render build/bench

build/core/%.o: src/core/%.cpp $(wildcard src/core/*.h)
	@mkdir -p build/core
	$(CXX) $(CXXFLAGS) -c $< -o $@

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
