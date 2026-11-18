// Measures whether Metal helps, instead of assuming it. Two workloads:
//   - a small 2D update the size of the soundboard, one dispatch per time step, which is
//     what coupling to the strings every sample would require;
//   - a 3D air volume (7 point wave stencil), where the GPU has enough work per dispatch.
// Each is timed on the GPU (many steps per command buffer) and on one CPU core.
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <chrono>
#include <cstdio>
#include <vector>

static const char* kSource = R"(
#include <metal_stdlib>
using namespace metal;
struct Dims { uint nx, ny, nz; float c2; };
kernel void wave(device const float* prev [[buffer(0)]], device const float* cur [[buffer(1)]],
                 device float* next [[buffer(2)]], constant Dims& d [[buffer(3)]],
                 uint3 id [[thread_position_in_grid]]) {
    if (id.x >= d.nx || id.y >= d.ny || id.z >= d.nz) return;
    uint i = id.x + d.nx * (id.y + d.ny * id.z);
    if (id.x == 0 || id.y == 0 || id.x == d.nx - 1 || id.y == d.ny - 1 ||
        (d.nz > 1 && (id.z == 0 || id.z == d.nz - 1))) { next[i] = 0; return; }
    float lap = cur[i - 1] + cur[i + 1] + cur[i - d.nx] + cur[i + d.nx] - 4.0f * cur[i];
    if (d.nz > 1) lap += cur[i - d.nx * d.ny] + cur[i + d.nx * d.ny] - 2.0f * cur[i];
    next[i] = 2.0f * cur[i] - prev[i] + d.c2 * lap;
}
)";

struct Dims { uint32_t nx, ny, nz; float c2; };

static double cpuStep(int nx, int ny, int nz, int steps) {
    size_t n = (size_t)nx * ny * nz;
    std::vector<float> a(n, 0), b(n, 0), c(n, 0);
    b[n / 2 + nx / 2] = 1;
    float c2 = nz > 1 ? 0.3f : 0.45f;
    float *prev = a.data(), *cur = b.data(), *next = c.data();
    auto t0 = std::chrono::steady_clock::now();
    for (int s = 0; s < steps; s++) {
        for (int z = (nz > 1 ? 1 : 0); z < (nz > 1 ? nz - 1 : 1); z++)
            for (int y = 1; y < ny - 1; y++) {
                const float* __restrict cu = cur + (size_t)nx * (y + (size_t)ny * z);
                const float* __restrict pr = prev + (size_t)nx * (y + (size_t)ny * z);
                float* __restrict ne = next + (size_t)nx * (y + (size_t)ny * z);
                const ptrdiff_t sy = nx, sz = (ptrdiff_t)nx * ny;
                if (nz > 1)
                    for (int x = 1; x < nx - 1; x++)
                        ne[x] = 2 * cu[x] - pr[x] + c2 * (cu[x - 1] + cu[x + 1] + cu[x - sy] + cu[x + sy] + cu[x - sz] + cu[x + sz] - 6 * cu[x]);
                else
                    for (int x = 1; x < nx - 1; x++)
                        ne[x] = 2 * cu[x] - pr[x] + c2 * (cu[x - 1] + cu[x + 1] + cu[x - sy] + cu[x + sy] - 4 * cu[x]);
            }
        float* t = prev; prev = cur; cur = next; next = t;
    }
    return std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count() / steps;
}

int main() {
    @autoreleasepool {
        id<MTLDevice> dev = MTLCreateSystemDefaultDevice();
        if (!dev) { printf("no Metal device\n"); return 1; }
        NSError* err = nil;
        id<MTLLibrary> lib = [dev newLibraryWithSource:@(kSource) options:nil error:&err];
        if (!lib) { printf("shader compile failed: %s\n", err.localizedDescription.UTF8String); return 1; }
        id<MTLComputePipelineState> pipe = [dev newComputePipelineStateWithFunction:[lib newFunctionWithName:@"wave"] error:&err];
        id<MTLCommandQueue> queue = [dev newCommandQueue];
        printf("GPU: %s\n", dev.name.UTF8String);
        printf("%-26s %10s %14s %14s %12s %12s\n", "grid", "cells", "GPU us/step", "CPU us/step", "GPU Mcell/s", "CPU Mcell/s");

        struct Case { int nx, ny, nz, steps; const char* note; };
        Case cases[] = {{64, 58, 1, 512, "board sized, 2D"}, {256, 256, 1, 256, "2D"}, {64, 64, 64, 128, "air, 0.8 m cube @12 mm"},
                        {128, 128, 128, 64, "air, 1.6 m cube"}, {256, 256, 128, 32, "air, 3.2 x 3.2 x 1.6 m"}, {320, 256, 256, 16, "air, room 4 x 3.2 x 3.2 m"}};
        for (const Case& cs : cases) {
            size_t n = (size_t)cs.nx * cs.ny * cs.nz;
            id<MTLBuffer> buf[3];
            for (auto& b : buf) { b = [dev newBufferWithLength:n * 4 options:MTLResourceStorageModeShared]; memset(b.contents, 0, n * 4); }
            ((float*)buf[1].contents)[n / 2 + cs.nx / 2] = 1;
            Dims d{(uint32_t)cs.nx, (uint32_t)cs.ny, (uint32_t)cs.nz, cs.nz > 1 ? 0.3f : 0.45f};
            MTLSize grid = MTLSizeMake(cs.nx, cs.ny, cs.nz);
            NSUInteger w = pipe.threadExecutionWidth, h = pipe.maxTotalThreadsPerThreadgroup / w;
            MTLSize group = cs.nz > 1 ? MTLSizeMake(w, h >= 4 ? h / 4 : 1, h >= 4 ? 4 : 1) : MTLSizeMake(w, h, 1);
            double best = 1e9;
            for (int rep = 0; rep < 4; rep++) {
                auto t0 = std::chrono::steady_clock::now();
                id<MTLCommandBuffer> cb = [queue commandBuffer];
                id<MTLComputeCommandEncoder> enc = [cb computeCommandEncoder];
                [enc setComputePipelineState:pipe];
                [enc setBytes:&d length:sizeof(d) atIndex:3];
                for (int s = 0; s < cs.steps; s++) {
                    [enc setBuffer:buf[s % 3] offset:0 atIndex:0];
                    [enc setBuffer:buf[(s + 1) % 3] offset:0 atIndex:1];
                    [enc setBuffer:buf[(s + 2) % 3] offset:0 atIndex:2];
                    [enc dispatchThreads:grid threadsPerThreadgroup:group];
                }
                [enc endEncoding];
                [cb commit];
                [cb waitUntilCompleted];
                best = std::min(best, std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count() / cs.steps);
            }
            double cpu = 1e9;
            for (int rep = 0; rep < 2; rep++) cpu = std::min(cpu, cpuStep(cs.nx, cs.ny, cs.nz, std::max(4, cs.steps / 4)));
            char name[64];
            snprintf(name, sizeof(name), "%dx%dx%d", cs.nx, cs.ny, cs.nz);
            printf("%-26s %10zu %14.1f %14.1f %12.0f %12.0f   %s\n", name, n, best * 1e6, cpu * 1e6, n / best / 1e6, n / cpu / 1e6, cs.note);
        }
        printf("\nOne audio sample at 48 kHz is 20.8 us. A step that must exchange data with the strings\n"
               "every sample also pays a CPU-GPU round trip, measured here for an empty command buffer:\n");
        double rt = 1e9;
        for (int rep = 0; rep < 50; rep++) {
            auto t0 = std::chrono::steady_clock::now();
            id<MTLCommandBuffer> cb = [queue commandBuffer];
            id<MTLComputeCommandEncoder> enc = [cb computeCommandEncoder];
            [enc endEncoding];
            [cb commit];
            [cb waitUntilCompleted];
            rt = std::min(rt, std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count());
        }
        printf("  commit + wait round trip: %.0f us (best of 50)\n", rt * 1e6);
    }
    return 0;
}
