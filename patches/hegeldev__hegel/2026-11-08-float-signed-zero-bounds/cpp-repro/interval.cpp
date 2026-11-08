#include <hegel/hegel.h>
#include <stdexcept>
#include <utility>

namespace gs = hegel::generators;

struct Interval {
    double lo, hi;
    bool contains(double x) const { return lo <= x && x <= hi; }
};

HEGEL_TEST(point_drawn_in_interval_is_contained, {.seed = 71, .database = hegel::Database::disabled()})(hegel::TestCase& tc) {
    auto bound = gs::floats<double>({.min_value = -1e6, .max_value = 1e6});
    double lo = tc.draw("lo", bound);
    double hi = tc.draw("hi", bound);
    if (hi < lo) std::swap(lo, hi);
    Interval iv{lo, hi};

    double x = tc.draw("x", gs::floats<double>({.min_value = iv.lo, .max_value = iv.hi}));
    if (!iv.contains(x)) throw std::runtime_error("point outside its interval");
}

int main() { point_drawn_in_interval_is_contained(); }
