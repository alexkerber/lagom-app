#!/usr/bin/env python3
"""Check the budget runner's parsing and pass/fail rule without running Cargo."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("perf", Path(__file__).with_name("perf.py"))
perf = importlib.util.module_from_spec(spec)
spec.loader.exec_module(perf)


class Perf(unittest.TestCase):
    def test_reads_lines_after_libtest_prefix(self):
        out = "test perf_tests::a ... perf mime.typical 0.393\nperf search.from 12\nnot perf x 1\nperf bad 1 extra\n"
        self.assertEqual(perf.measurements(out), {"mime.typical": 0.393, "search.from": 12.0})

    def test_fails_over_limit_and_when_not_measured(self):
        budgets = {"a": {"max": 10, "unit": "ms"}, "b": {"max": 10, "unit": "ms"}}
        self.assertTrue(perf.report(budgets, 2.0, {"a": 20.0, "b": 1.0, "extra": 5.0})[1])
        self.assertFalse(perf.report(budgets, 2.0, {"a": 20.1, "b": 1.0})[1])
        table, ok = perf.report(budgets, 2.0, {"a": 1.0})
        self.assertFalse(ok)
        self.assertIn("| b | not measured | 10 ms | 20 ms | FAIL |", table)
        self.assertFalse(perf.report({"size": {"max": 30, "unit": "MB"}}, 2.0, {"size": 30.1})[1])
        linux = {"size": {"max": 30, "max_linux": 55, "unit": "MB"}}
        self.assertTrue(perf.report(linux, 2.0, {"size": 53.2}, "linux")[1])
        self.assertFalse(perf.report(linux, 2.0, {"size": 53.2}, "darwin")[1])

    def test_windows_binary_size_budget(self):
        config = perf.tomllib.loads((perf.ROOT / "docs/performance-budgets.toml").read_text(encoding="utf-8"))
        budgets = {"binary.size": config["budgets"]["binary.size"]}
        for size, expected in [(32, True), (32 + 1e-6, False)]:
            with self.subTest(size=size):
                self.assertEqual(perf.report(budgets, config["tolerance"], {"binary.size": size}, "win32")[1], expected)
        times = {"sync": {"max": 10, "unit": "ms"}}
        self.assertTrue(perf.report(times, 2.0, {"sync": 9000.0}, "win32")[1])
        self.assertFalse(perf.report(times, 2.0, {"sync": 9000.0}, "linux")[1])


if __name__ == "__main__":
    unittest.main()
