#include <hegel/hegel.h>
#include <cstdio>
#include <exception>

namespace gs = hegel::generators;

HEGEL_TEST(signed_zero_bounds)(hegel::TestCase& tc) {
    tc.draw("x", gs::floats<double>({.min_value = 0.0, .max_value = -0.0}));
}

int main() {
    try {
        signed_zero_bounds();
        std::puts("no error");
    } catch (const std::exception& e) {
        std::printf("%s\n", e.what());
    }
}
