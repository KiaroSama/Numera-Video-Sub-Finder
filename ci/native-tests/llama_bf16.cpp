// Tests the linked ggml-cpu kernel, not a copied implementation of its SIMD path.
#include "ggml.h"
#include "ggml-cpu.h"

#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <limits>
#include <stdexcept>
#include <string>
#include <vector>
#ifdef _WIN32
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#else
#include <sys/utsname.h>
#endif

// Pinned vec.h:53-72 declares this symbol inside extern "C"; vec.cpp inherits that linkage.
extern "C" void ggml_vec_dot_bf16(int n, float * s, size_t bs, ggml_bf16_t * x,
                      size_t bx, ggml_bf16_t * y, size_t by, int nrc);

static_assert(sizeof(ggml_bf16_t) == sizeof(uint16_t));
static_assert(sizeof(float) == sizeof(uint32_t));
static_assert(std::numeric_limits<float>::is_iec559);

static float widen(uint16_t bits) {
    const uint32_t word = uint32_t(bits) << 16;
    float result;
    std::memcpy(&result, &word, sizeof(result));
    return result;
}

static ggml_bf16_t stored(uint16_t bits) {
    ggml_bf16_t value;
    std::memcpy(&value, &bits, sizeof(value));
    return value;
}

static std::string runtime_arch() {
#ifdef _WIN32
    SYSTEM_INFO info{};
    GetNativeSystemInfo(&info);
    if (info.wProcessorArchitecture == PROCESSOR_ARCHITECTURE_ARM64) return "aarch64";
    if (info.wProcessorArchitecture == PROCESSOR_ARCHITECTURE_AMD64) return "x86_64";
    return "unsupported";
#else
    utsname info{};
    if (uname(&info) != 0) throw std::runtime_error("Cannot inspect actual runtime architecture");
    std::string arch(info.machine);
    return arch == "arm64" ? "aarch64" : arch;
#endif
}

static void fill(int pattern, int i, uint16_t & x, uint16_t & y) {
    constexpr std::array<uint16_t, 8> ordinary{0x3f80, 0xbf80, 0x4000, 0xc040, 0x3e80, 0xbe00, 0x4120, 0xc0c0};
    switch (pattern) {
    case 0: x = 0; y = ordinary[size_t(i) % ordinary.size()]; break;
    case 1: x = ordinary[size_t(i) % ordinary.size()]; y = ordinary[(size_t(i) * 3 + 1) % ordinary.size()]; break;
    case 2: // Equal and opposite products stress cancellation without changing the oracle.
        x = (i % 2 == 0) ? 0x3f80 : 0xbf80; y = 0x3f80; break;
    case 3: // BF16 subnormals widen exactly; multiplication by1 is exact even in FP32.
        x = uint16_t(1 + i % 127); y = 0x3f80; break;
    case 4: // Finite exponent spread, bounded products: avoid NaN/overflow assumptions.
        x = uint16_t(((67 + i % 121) << 7) | (i % 128));
        y = uint16_t(((187 - i % 121) << 7) | ((i * 7) % 128));
        if (i % 3 == 0) x |= 0x8000;
        break;
    case 5: // Small residual after cancellation; mixed tails cannot be ignored.
        x = (i % 4 == 0) ? 0x3f81 : ((i % 2 == 0) ? 0x3f80 : 0xbf80);
        y = 0x3f80; break;
    default: throw std::runtime_error("Unknown numeric fixture");
    }
}

int main(int argc, char ** argv) {
    try {
        if (argc != 2 || (std::string(argv[1]) != "--require-arm64" && std::string(argv[1]) != "--native")) {
            throw std::runtime_error("Usage: nvsf-llama-bf16 --require-arm64|--native");
        }
        const auto arch = runtime_arch();
        const bool require_arm = std::string(argv[1]) == "--require-arm64";
#if defined(__aarch64__) && defined(__ARM_NEON)
        constexpr bool compiled_arm = true;
#else
        constexpr bool compiled_arm = false;
#endif
        if (require_arm && (!compiled_arm || arch != "aarch64" || ggml_cpu_has_neon() != 1)) {
            throw std::runtime_error("ARM64 witness needs actual native ARM64 and linked runtime NEON support");
        }
        if (arch != "aarch64" && arch != "x86_64") throw std::runtime_error("Unsupported actual runtime architecture");
        const auto started = std::chrono::steady_clock::now();
        const std::array<int, 13> lengths{0, 1, 7, 8, 9, 15, 16, 17, 31, 32, 33, 257, 4608};
        size_t cases = 0;
        double worst_fraction = 0;
        size_t unaligned_cases = 0;
        for (const int n : lengths) for (int offset = 0; offset < 4; ++offset) for (int pattern = 0; pattern < 6; ++pattern) {
            if (std::chrono::steady_clock::now() - started > std::chrono::seconds(5)) throw std::runtime_error("BF16 witness exceeded5-second internal deadline");
            const size_t count = size_t(n) + size_t(offset) + 8;
            std::vector<ggml_bf16_t> xs(count, stored(0x4a7b)), ys(count, stored(0xc91d));
            auto * x = xs.data() + offset;
            auto * y = ys.data() + offset;
            if (n >= 8 && (reinterpret_cast<uintptr_t>(x) % 16 != 0 || reinterpret_cast<uintptr_t>(y) % 16 != 0)) ++unaligned_cases;
            double expected = 0, absolute_sum = 0;
            for (int i = 0; i < n; ++i) {
                uint16_t a, b;
                fill(pattern, i, a, b);
                x[i] = stored(a); y[i] = stored(b);
                const double product = double(widen(a)) * double(widen(b));
                expected += product;
                absolute_sum += std::abs(product);
            }
            const auto x_before = xs, y_before = ys;
            std::array<float, 3> output{-123.0f, std::numeric_limits<float>::quiet_NaN(), 456.0f};
            ggml_vec_dot_bf16(n, &output[1], 0, x, 0, y, 0, 1);
            // Unit roundoff bound: FP32 FMA lanes + reduction/final rounding, against double oracle.
            // Use n+16 on non-ARM kernels whose reduction topology differs; ARM has8 independent lanes.
            const double u = double(std::numeric_limits<float>::epsilon()) / 2;
            const double steps = (compiled_arm && require_arm) ? double((n + 7) / 8 + 16) : double(n + 16);
            const double gamma = steps * u / (1 - steps * u);
            const double allowance = gamma * absolute_sum + double(n + 1) * double(std::numeric_limits<float>::denorm_min());
            const double error = std::abs(double(output[1]) - expected);
            if (!std::isfinite(output[1]) || error > allowance || output[0] != -123.0f || output[2] != 456.0f ||
                std::memcmp(xs.data(), x_before.data(), xs.size() * sizeof(ggml_bf16_t)) != 0 ||
                std::memcmp(ys.data(), y_before.data(), ys.size() * sizeof(ggml_bf16_t)) != 0) {
                std::cerr << "BF16 mismatch n=" << n << " offset=" << offset << " pattern=" << pattern
                          << " actual=" << output[1] << " expected=" << expected << " bound=" << allowance << '\n';
                throw std::runtime_error("Actual compiled BF16 kernel failed independent scalar oracle or buffer preservation");
            }
            if (allowance > 0) worst_fraction = std::max(worst_fraction, error / allowance);
            ++cases;
        }
        if (unaligned_cases == 0) throw std::runtime_error("No actual unaligned SIMD storage was exercised");
        std::cout << "{\"schema\":1,\"passed\":true,\"tests\":" << cases
                  << ",\"failed\":0,\"skipped\":0,\"runtime_arch\":\"" << arch
                  << "\",\"neon\":" << ggml_cpu_has_neon() << ",\"arm_fma\":" << ggml_cpu_has_arm_fma()
                  << ",\"max_error_fraction\":" << worst_fraction << ",\"oracle\":\"independent BF16 bit-widen/double-product-sum\"}\n";
        return 0;
    } catch (const std::exception & error) {
        std::cerr << "BF16 kernel witness failed: " << error.what() << '\n';
        return 1;
    }
}
