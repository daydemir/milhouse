#!/usr/bin/env python3
"""Run a long command once, detached from the agent that launched it."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def write_json(path: Path, value: dict) -> None:
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(value) + "\n")
    tmp.replace(path)


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def status(state: Path) -> dict:
    result = state / "result.json"
    if result.exists():
        return {"state": "completed", **json.loads(result.read_text())}
    pid_file = state / "pid"
    if not pid_file.exists():
        return {"state": "not_started"}
    pid = int(pid_file.read_text())
    return {"state": "running" if alive(pid) else "lost", "pid": pid}


def worker(state: Path, command: list[str]) -> int:
    with (state / "output.log").open("ab", buffering=0) as output:
        completed = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.STDOUT)
    write_json(state / "result.json", {"exit_code": completed.returncode})
    return completed.returncode


def start(state: Path, command: list[str]) -> dict:
    state.mkdir(parents=True, exist_ok=True)
    current = status(state)
    if current["state"] != "not_started":
        return current
    child = subprocess.Popen(
        [sys.executable, __file__, "_worker", "--state", str(state), "--", *command],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
        close_fds=True,
    )
    (state / "pid").write_text(f"{child.pid}\n")
    return {"state": "running", "pid": child.pid}


def self_test() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        state = Path(tmp)
        assert start(state, ["sh", "-c", "sleep .1; echo ok"])["state"] == "running"
        for _ in range(30):
            if status(state)["state"] == "completed":
                break
            time.sleep(.1)
        assert status(state) == {"state": "completed", "exit_code": 0}
        assert (state / "output.log").read_text().strip() == "ok"


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="action", required=True)
    for action in ["start", "status", "_worker"]:
        command = sub.add_parser(action)
        command.add_argument("--state", type=Path, required=True)
        if action != "status":
            command.add_argument("command", nargs=argparse.REMAINDER)
    sub.add_parser("self-test")
    args = parser.parse_args()
    if args.action == "self-test":
        self_test()
        print('{"ok": true}')
        return 0
    if args.action == "status":
        print(json.dumps(status(args.state)))
        return 0
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("command required after --")
    if args.action == "_worker":
        return worker(args.state, command)
    print(json.dumps(start(args.state, command)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
