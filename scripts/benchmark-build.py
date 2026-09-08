#!/usr/bin/env python3
"""Measure real offline builds with an installed system TeX Live and a command suite."""

import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time


def prepare(root, suite):
    root.mkdir()
    suffix = ".exe" if os.name == "nt" else ""
    binaries = {name: suite / (name + suffix) for name in ("texe", "pqty", "pqty-fls")}
    for binary in binaries.values():
        if not binary.is_file():
            raise ValueError(f"Missing suite binary: {binary}")
    (root / "texe.toml").write_text(
        'schema = "texe.project/v1"\n[project]\nentry = "main.tex"\n'
        '[toolchain]\nprovider = "system"\nengine = "pdflatex"\n'
        '[packages]\nremote = false\n'
        f'manager = {json.dumps(str(binaries["pqty"]))}\n'
        f'trace_adapter = {json.dumps(str(binaries["pqty-fls"]))}\n',
        encoding="utf-8",
    )
    environment = dict(os.environ, TEXE_HOME=str(root.with_name(root.name + "-home")),
                       XDG_CACHE_HOME=str(root.with_name(root.name + "-cache")))
    return binaries["texe"], environment


def document(iteration, fixture):
    if fixture == "references":
        sections = "".join(
            f"\\section{{Section {number}}}\\label{{sec:{number}}}\n"
            f"Editing benchmark {iteration}. See Section~\\ref{{sec:12}}.\n"
            r"\[\sum_{k=1}^{n} k = \frac{n(n+1)}{2}\]" + "\n\\clearpage\n"
            for number in range(1, 13)
        )
        return ("\\documentclass{article}\n\\usepackage{amsmath,hyperref}\n"
                "\\begin{document}\n\\tableofcontents\n\\clearpage\n"
                + sections + "\\end{document}\n")
    return ("\\documentclass{article}\n\\begin{document}\n"
            f"Editing benchmark {iteration}.\n\\end{{document}}\n")


def measure(root, binary, environment, iteration, fixture):
    (root / "main.tex").write_text(document(iteration, fixture), encoding="utf-8")
    start = time.perf_counter()
    result = subprocess.run(
        [str(binary), "build", "--project", str(root), "--offline", "--json"],
        env=environment, capture_output=True, text=True, timeout=180,
    )
    elapsed = time.perf_counter() - start
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)
    report = json.loads(result.stdout)
    if report["cached"] or not (root / "main.pdf").is_file():
        raise RuntimeError("Expected a real system build and published PDF")
    return {"iteration": iteration, "wall_seconds": elapsed,
            "engine_passes": report["engine_passes"],
            "convergence_rounds": report["convergence_rounds"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("suite", type=Path, help="Directory containing optimized suite binaries")
    parser.add_argument("--baseline", type=Path, help="Optional previous suite to compare")
    parser.add_argument("--runs", type=int, default=5, help="Changed builds after the first build")
    parser.add_argument("--output", type=Path, help="Write raw measurements and medians as JSON")
    parser.add_argument("--fixture", choices=["simple", "references"], default="simple",
                        help="Small paper or a document with math, links and cross-references")
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    suites = {"candidate": args.suite.resolve()}
    if args.baseline:
        suites["baseline"] = args.baseline.resolve()
    results = {name: {"suite": str(suite), "fixture": args.fixture, "samples": []} for name, suite in suites.items()}
    with tempfile.TemporaryDirectory(prefix="texe-benchmark-") as temporary:
        roots = {name: Path(temporary) / name for name in suites}
        prepared = {name: prepare(roots[name], suite) for name, suite in suites.items()}
        # Alternate order to reduce bias from machine load and filesystem caches.
        for iteration in range(args.runs + 1):
            names = list(suites) if iteration % 2 else list(reversed(suites))
            for name in names:
                row = measure(roots[name], *prepared[name], iteration, args.fixture)
                results[name]["samples"].append(row)
                print(f"{name}: {json.dumps(row)}", flush=True)
    for name, result in results.items():
        result["edit_median_seconds"] = statistics.median(
            row["wall_seconds"] for row in result["samples"][1:]
        )
        print(f'{name} edit median: {result["edit_median_seconds"]:.3f}s')
    if args.output:
        args.output.write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
