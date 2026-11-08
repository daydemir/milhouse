---
name: milhouse
description: "Execute a complete contract.md end-to-end through the Milhouse CLI: frozen contract, Codex loop, verification, grader, result.json, and ledger."
---

# /milhouse — a contract-driven task runner

The CLI owns the run lifecycle. You launch it with a contract, then report from its `result.json`. Do not
reimplement the run inline.

## 1. Pick the invocation

- Contract: `mil build --workdir /path/to/repo --contract contract.md --id task-123`
- Interactive contract planning: `mil plan --workdir /path/to/repo --main opus-5-5-high-subscription --advisor sol-6.1-high-subscription "make the map load in <1s"`
- Cancel stale/active run: `mil cancel task-123`

Task-source selection and scheduling belong to the caller; the core CLI does not know any tracker.

## 2. Long runs

A real run can take 10+ minutes. If your shell has a short timeout, launch detached:

```bash
nohup mil build --workdir /path/to/repo --contract contract.md --id task-123 >/tmp/mil-launch.log 2>&1 &
```

Watch progress in the newest run under `~/.milhouse/runs/*/milhouse.log`.

## 3. Report on exit

Read `<run_dir>/result.json` and report: **status**, commit, cost, models, run_dir, and reason. Dashboard:
`mil dashboard` (`http://127.0.0.1:8471/`).

If failed/blocked: quote `reason` plus the relevant `milhouse.log` tail. Do not auto-retry; the rails already retried.

Task: $ARGUMENTS
