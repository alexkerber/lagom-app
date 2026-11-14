#!/usr/bin/env python3
"""Run the headless budget benchmarks in release and compare them with docs/performance-budgets.toml.

Prints a Markdown table, appends it to $GITHUB_STEP_SUMMARY when set, and exits 1 when a
measurement is over budget × tolerance or a budgeted key was not measured.
"""
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parent.parent
BENCHES = [
    ["cargo", "test", "--release", "--locked", "-p", "lagom-core", "--lib", "perf_tests",
     "--", "--ignored", "--nocapture", "--test-threads=1"],
    ["cargo", "test", "--release", "--locked", "-p", "lagom", "--bin", "lagom",
     "--", "--ignored", "--nocapture", "--test-threads=1", "reload_reads_100k", "reload_ui_thread_100k"],
]
# libtest prints "test name ... " without a newline, so a line can start with that.
LINE = re.compile(r"(?:^|\.\.\. )perf (\S+) (\d+(?:\.\d+)?)$")


def measurements(output):
    return {m[1]: float(m[2]) for line in output.splitlines() if (m := LINE.search(line))}


def report(budgets, tolerance, measured):
    """The Markdown table and whether every budget holds."""
    rows = ["| Key | Measured | Budget | Fails above | Status |", "| --- | ---: | ---: | ---: | --- |"]
    ok = True
    for key in sorted(budgets.keys() | measured.keys()):
        value, budget = measured.get(key), budgets.get(key)
        unit = budget["unit"] if budget else ""
        shown = "not measured" if value is None else f"{value:.3f} {unit}".strip()
        if budget is None:
            rows.append(f"| {key} | {shown} | – | – | no budget |")
            continue
        # Sizes do not vary between runners; only times get the tolerance.
        limit = budget["max"] * (tolerance if unit == "ms" else 1)
        status = "ok" if value is not None and value <= limit else "FAIL"
        ok &= status == "ok"
        rows.append(f"| {key} | {shown} | {budget['max']} {unit} | {limit:g} {unit} | {status} |")
    return "\n".join(rows), ok


def main():
    config = tomllib.loads((ROOT / "docs/performance-budgets.toml").read_text())
    tolerance = float(os.environ.get("PERF_TOLERANCE", config["tolerance"]))
    measured, failed = {}, []
    for command in BENCHES:
        run = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        sys.stdout.write(run.stdout)
        if run.returncode:
            failed.append(" ".join(command))
        measured |= measurements(run.stdout)
    build = subprocess.run(["cargo", "build", "--release", "--locked", "-p", "lagom"], cwd=ROOT)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    binary = target / "release" / ("lagom.exe" if os.name == "nt" else "lagom")
    if build.returncode:
        failed.append("cargo build --release -p lagom")
    else:
        measured["binary.size"] = binary.stat().st_size / 1e6
    table, ok = report(config["budgets"], tolerance, measured)
    summary = f"## Performance budgets ({sys.platform})\n\nTolerance ×{tolerance:g}; best of 5 runs. Budgets: `docs/performance-budgets.toml`.\n\n{table}\n"
    if failed:
        summary += "\nCommands that failed:\n" + "".join(f"\n- `{c}`" for c in failed) + "\n"
    print(summary)
    if path := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(path, "a") as f:
            f.write(summary)
    return 0 if ok and not failed else 1


if __name__ == "__main__":
    sys.exit(main())
