#!/usr/bin/env python3
from __future__ import annotations

import json
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent
CLI = ROOT / "beatmerge.py"
ROUTE = "37.0,-122.0;37.5,-122.0"


class VerifyError(Exception):
    pass


def run_cli(sources: list[Path], extra: list[str] | None = None, timeout: int = 10) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["python3", str(CLI), "--sources", *(str(source) for source in sources), "--route", ROUTE, *(extra or [])],
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def require(condition: bool, message: str) -> None:
    if not condition:
        raise VerifyError(message)


def load_stdout_json(proc: subprocess.CompletedProcess[str], label: str) -> list[object]:
    require(proc.returncode == 0, f"{label}: exit {proc.returncode}, stderr={proc.stderr[:200]!r}")
    try:
        data = json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        raise VerifyError(f"{label}: stdout is not JSON: {exc}") from exc
    require(isinstance(data, list), f"{label}: stdout JSON is not an array")
    return data


def write_jsonl(path: Path, rows: int, row_for_index) -> None:
    with path.open("w") as fh:
        for index in range(rows):
            fh.write(json.dumps(row_for_index(index)) + "\n")


def check_worst_case_perf() -> None:
    with tempfile.TemporaryDirectory() as td:
        path = Path(td) / "worst.jsonl"
        write_jsonl(path, 10_000, lambda i: {"title": "Same Title", "lat": 37.0 + i * 0.01, "lng": -122.0})
        started = time.perf_counter()
        proc = run_cli([path], ["--on-error", "skip"])
        elapsed = time.perf_counter() - started
    data = load_stdout_json(proc, "worst-case perf")
    require(len(data) == 10_000, f"worst-case perf: expected 10000 survivors, got {len(data)}")
    require(elapsed < 2.0, f"worst-case perf: took {elapsed:.3f}s")
    print(f"OK worst-case perf {elapsed:.3f}s")


def check_all_duplicates_perf() -> None:
    with tempfile.TemporaryDirectory() as td:
        path = Path(td) / "dups.jsonl"
        write_jsonl(path, 10_000, lambda _: {"title": "Same Title", "lat": 37.5, "lng": -122.0})
        started = time.perf_counter()
        proc = run_cli([path], ["--on-error", "skip"])
        elapsed = time.perf_counter() - started
    data = load_stdout_json(proc, "all-duplicates perf")
    require(len(data) == 1, f"all-duplicates perf: expected 1 survivor, got {len(data)}")
    require(elapsed < 2.0, f"all-duplicates perf: took {elapsed:.3f}s")
    print(f"OK all-duplicates perf {elapsed:.3f}s")


def check_fatal_path() -> None:
    with tempfile.TemporaryDirectory() as td:
        path = Path(td) / "broken.json"
        path.write_text("{broken")
        modes = (["--on-error", "fail"], ["--on-error", "skip"], [])
        for mode in modes:
            proc = run_cli([path], mode)
            label = "omitted" if not mode else mode[-1]
            require(proc.returncode != 0, f"fatal path {label}: expected nonzero exit")
            require(proc.stdout == "", f"fatal path {label}: expected empty stdout, got {proc.stdout[:80]!r}")
    print("OK fatal path")


def check_default_fail() -> None:
    sources = [ROOT / "fixtures" / "beats.json", ROOT / "fixtures" / "beats.csv", ROOT / "fixtures" / "beats.jsonl"]
    proc = run_cli(sources, [])
    require(proc.returncode != 0, "default-fail: expected nonzero exit")
    require(proc.stdout == "", f"default-fail: expected empty stdout, got {proc.stdout[:80]!r}")
    print("OK default fail")


def check_docs() -> None:
    readme = (ROOT / "README.md").read_text()
    lower = readme.lower()
    require("segment" in lower, "docs: missing 'segment'")
    require("tie" in lower, "docs: missing 'tie'")
    require(re.search(r"^.*default.*fail.*$", readme, re.IGNORECASE | re.MULTILINE) is not None, "docs: missing default.*fail line")
    print("OK docs")


def main() -> int:
    try:
        check_worst_case_perf()
        check_all_duplicates_perf()
        check_fatal_path()
        check_default_fail()
        check_docs()
    except VerifyError as exc:
        print(str(exc), file=sys.stderr)
        return 1
    print("VERIFY-OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
