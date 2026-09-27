#!/usr/bin/env python3
"""Splits cases.json into slices for parallel Neovim runs, and joins the
results back in order.

  split.py split CASES.json DIR N   → DIR/cases-000.json ...
  split.py join DIR EXPECTED.json   ← DIR/cases-000.out.json ...
"""
import json, pathlib, sys

if sys.argv[1] == "split":
    cases = json.loads(pathlib.Path(sys.argv[2]).read_text())
    n = max(1, int(sys.argv[4]))
    size = -(-len(cases) // n)
    for i in range(n):
        part = cases[i * size:(i + 1) * size]
        if part:
            pathlib.Path(sys.argv[3], f"cases-{i:03}.json").write_text(json.dumps(part, ensure_ascii=False))
else:
    results = []
    for f in sorted(pathlib.Path(sys.argv[2]).glob("cases-*.out.json")):
        results += json.loads(f.read_text())
    pathlib.Path(sys.argv[3]).write_text(json.dumps(results, ensure_ascii=False) + "\n")
    print(f"{len(results)} cases run")
