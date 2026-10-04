"""Big-test comparison: large examples, rare bugs, the shrink that follows."""
import sys, time
from hypothesis import given, settings, strategies as st, HealthCheck, Phase, seed
from hypothesis.errors import Unsatisfiable, Flaky

MODE = sys.argv[1]
record = st.fixed_dictionaries({
    "id": st.integers(0, 2**31), "name": st.text(min_size=1, max_size=12),
    "score": st.floats(0, 100, allow_nan=False), "tags": st.lists(st.sampled_from(["a", "b", "c", "d"]), max_size=4),
    "active": st.booleans(),
})

def gen(name, strategy, n=300):
    calls = 0; draws = 0
    @settings(database=None, max_examples=n, deadline=None, phases=[Phase.generate], suppress_health_check=list(HealthCheck), derandomize=True)
    @given(strategy)
    def f(x):
        nonlocal calls, draws
        calls += 1; draws += len(x)
    t = time.perf_counter(); f(); dt = time.perf_counter() - t
    print(f"{MODE:9s} gen  {name:34s} {1000*dt/calls:7.2f} ms/ex  avg len {draws/calls:6.1f}")

def hunt(name, strategy, bug, expected, max_examples=20000, seeds=(0, 1, 2)):
    for sd in seeds:
        calls = 0; first = None; t_first = None; last_failing = [None]
        @seed(sd)
        @settings(database=None, max_examples=max_examples, deadline=None, suppress_health_check=list(HealthCheck), report_multiple_bugs=False)
        @given(strategy)
        def f(x):
            nonlocal calls, first, t_first
            calls += 1
            if bug(x):
                if first is None: first = calls; t_first = time.perf_counter() - t0
                last_failing[0] = x
                raise AssertionError
        t0 = time.perf_counter()
        try:
            f(); res = "NOT FOUND"
        except AssertionError:
            res = "found"
        except Unsatisfiable:
            res = "unsatisfiable"
        dt = time.perf_counter() - t0
        if first is None:
            print(f"{MODE:9s} hunt {name:28s} seed {sd}  {res} after {calls} calls, {dt:.1f} s"); continue
        ok = "min ok" if expected(last_failing[0]) else f"NOT minimal: {str(last_failing[0])[:60]}"
        print(f"{MODE:9s} hunt {name:28s} seed {sd}  found@{first:5d} ({t_first:5.1f} s)  shrink {calls-first:5d} calls {dt-t_first:6.1f} s  {ok}")

gen("records x100..200", st.lists(record, min_size=100, max_size=200))
gen("unique-id records x100..200", st.lists(record, min_size=100, max_size=200, unique_by=lambda r: r["id"]))
gen("matrix 30x30 ints", st.lists(st.lists(st.integers(), min_size=30, max_size=30), min_size=30, max_size=30))

pairs = st.lists(st.tuples(st.integers(0, 1000), st.text(min_size=1)))
hunt("needle pair >990 & len>=3", pairs,
     lambda xs: any(a > 990 and len(s) >= 3 for a, s in xs),
     lambda xs: xs == [(991, "000")])
hunt("duplicate id, different name", st.lists(record, min_size=2),
     lambda rs: any(a["id"] == b["id"] and a["name"] != b["name"] for i, a in enumerate(rs) for b in rs[i+1:]),
     lambda rs: len(rs) == 2 and rs[0]["id"] == rs[1]["id"] == 0 and {rs[0]["name"], rs[1]["name"]} == {"0", "1"})
hunt("nested sum > 10**6 with a row >= 5", st.lists(st.lists(st.integers())),
     lambda xss: sum(map(sum, xss)) > 10**6 and any(len(xs) >= 5 for xs in xss),
     lambda xss: xss == [[0, 0, 0, 0, 1000001]] or (len(xss) == 1 and len(xss[0]) == 5 and sum(xss[0]) == 1000001))
hunt("sorted run of 6 in list", st.lists(st.integers(0, 100)),
     lambda xs: any(xs[i:i+6] == sorted(xs[i:i+6]) and len(set(xs[i:i+6])) == 6 for i in range(len(xs) - 5)),
     lambda xs: xs == [0, 1, 2, 3, 4, 5])
