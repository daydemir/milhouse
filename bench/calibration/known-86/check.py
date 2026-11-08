#!/usr/bin/env python3
"""Acceptance checks for the `beatmerge` CLI.

Runs `beatmerge.py` as a black box against the repo fixtures and asserts the contract from task.md.
Prints `PASS` and exits 0 only when every check passes; otherwise prints `FAIL k/N` (with per-check
reasons on stderr) and exits 1.

The empty stub prints nothing, so every CLI-dependent check fails on a fresh checkout — that is the
expected failing baseline.
"""
import json
import math
import os
import pathlib
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parent
CLI = ROOT / "beatmerge.py"
FX = ROOT / "fixtures"
SOURCES = [str(FX / "beats.json"), str(FX / "beats.csv"), str(FX / "beats.jsonl")]
# Route: the start vertex sits ~9 m from the "far" City Hall beat; the remaining vertices run
# through open water, all >200 m from every beat. So under any sensible path-proximity metric
# (nearest vertex, nearest point on a segment, arc-length projection) that "far" City Hall is the
# beat closest to the route and must come out first — the one invariant this check pins.
ROUTE = "37.80200,-122.26810;37.80300,-122.25900;37.80400,-122.25200"
ROUTE_START = (37.80200, -122.26810)

_results = []


def check(name, fn):
    try:
        ok, detail = fn()
    except Exception as exc:  # a throwing check is a failing check
        ok, detail = False, f"{type(exc).__name__}: {exc}"
    _results.append((name, ok, detail))


def run_cli(extra, sources=None, timeout=30):
    cmd = [sys.executable, str(CLI), "--sources", *(sources or SOURCES), "--route", ROUTE, *extra]
    return subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)


def haversine_m(a, b):
    r = 6371000.0
    p1, p2 = math.radians(a[0]), math.radians(b[0])
    dp = math.radians(b[0] - a[0])
    dl = math.radians(b[1] - a[1])
    h = math.sin(dp / 2) ** 2 + math.cos(p1) * math.cos(p2) * math.sin(dl / 2) ** 2
    return 2 * r * math.asin(math.sqrt(h))


# One cached happy-path run (--on-error skip) reused by most checks.
_HAPPY = {}


def happy():
    if not _HAPPY:
        proc = run_cli(["--on-error", "skip"])
        _HAPPY["proc"] = proc
        try:
            _HAPPY["data"] = json.loads(proc.stdout)
        except Exception:
            _HAPPY["data"] = None
    return _HAPPY


def titles(data):
    return [r.get("title") for r in data]


# ---- checks ---------------------------------------------------------------------------------

def c_runs():
    p = happy()["proc"]
    return p.returncode == 0, f"exit={p.returncode} stderr={p.stderr[:200]!r}"


def c_json():
    d = happy()["data"]
    return d is not None, "stdout is not valid JSON"


def c_is_list():
    d = happy()["data"]
    return isinstance(d, list), f"type={type(d).__name__}"


def c_typed_fields():
    d = happy()["data"] or []
    for r in d:
        if not isinstance(r.get("title"), str) or not r["title"]:
            return False, f"bad title: {r!r}"
        for k in ("lat", "lng"):
            v = r.get(k)
            if not isinstance(v, (int, float)) or isinstance(v, bool) or not math.isfinite(v):
                return False, f"bad {k}: {r!r}"
    return True, ""


def c_count():
    d = happy()["data"] or []
    return len(d) == 6, f"expected 6 survivors, got {len(d)}"


def c_dedup_lake():
    d = happy()["data"] or []
    n = titles(d).count("Lake Merritt")
    return n == 1, f"'Lake Merritt' appears {n}x (near-dup should collapse to 1)"


def c_dedup_rotary():
    d = happy()["data"] or []
    n = titles(d).count("Rotary Nature Center")
    return n == 1, f"'Rotary Nature Center' appears {n}x (near-dup should collapse to 1)"


def c_far_dup_kept():
    d = happy()["data"] or []
    n = titles(d).count("City Hall")
    return n == 2, f"'City Hall' appears {n}x (two are >25m apart — both must survive)"


def c_no_malformed():
    d = happy()["data"] or []
    bad = {"Bad", "Broken", "", None}
    leaked = [r for r in d if r.get("title") in bad]
    return not leaked, f"malformed rows leaked: {leaked!r}"


def c_route_first():
    d = happy()["data"] or []
    if not d:
        return False, "no output"
    dists = [haversine_m(ROUTE_START, (r["lat"], r["lng"])) for r in d]
    nearest = min(range(len(d)), key=lambda i: dists[i])
    return nearest == 0, f"first is {d[0]['title']!r}; nearest-to-start is idx {nearest} ({d[nearest]['title']!r})"


def c_fail_mode():
    # --on-error fail must exit nonzero on the malformed-containing fixtures and emit no JSON array.
    p = run_cli(["--on-error", "fail"])
    if p.returncode == 0:
        return False, "exit 0 despite malformed rows under --on-error fail"
    leaked = False
    try:
        leaked = isinstance(json.loads(p.stdout), list)
    except Exception:
        leaked = False
    return not leaked, "emitted a JSON array on stdout while failing"


def c_perf():
    with tempfile.TemporaryDirectory() as td:
        big = pathlib.Path(td) / "big.jsonl"
        with big.open("w") as fh:
            for i in range(10000):
                lat = 37.80 + (i % 500) * 0.0002
                lng = -122.27 + (i % 500) * 0.0002
                fh.write(json.dumps({"title": f"beat-{i}", "lat": lat, "lng": lng}) + "\n")
        t0 = time.perf_counter()
        p = run_cli(["--on-error", "skip"], sources=[str(big)], timeout=10)
        dt = time.perf_counter() - t0
    if p.returncode != 0:
        return False, f"CLI failed on 10k rows: {p.stderr[:200]!r}"
    return dt < 2.0, f"10k rows took {dt:.2f}s (bound: <2s)"


CHECKS = [
    ("cli runs (skip mode) exits 0", c_runs),
    ("stdout is valid JSON", c_json),
    ("output is a JSON array", c_is_list),
    ("records have typed title/lat/lng", c_typed_fields),
    ("6 survivors after dedupe", c_count),
    ("near-dup 'Lake Merritt' collapsed", c_dedup_lake),
    ("near-dup 'Rotary Nature Center' collapsed", c_dedup_rotary),
    ("far same-title 'City Hall' both kept", c_far_dup_kept),
    ("no malformed rows leaked", c_no_malformed),
    ("first beat is nearest to route start", c_route_first),
    ("--on-error fail exits nonzero, no partial JSON", c_fail_mode),
    ("10k rows merge in <2s", c_perf),
]


def main():
    for name, fn in CHECKS:
        check(name, fn)
    passed = sum(1 for _, ok, _ in _results if ok)
    total = len(_results)
    for name, ok, detail in _results:
        if not ok:
            print(f"  FAIL  {name}  --  {detail}", file=sys.stderr)
    if passed == total:
        print("PASS")
        sys.exit(0)
    print(f"FAIL {passed}/{total}")
    sys.exit(1)


if __name__ == "__main__":
    main()
