# Milhouse

Historical autonomous task harness. The prompt-only `climb` workflow is now
maintained in [dstack](https://github.com/daydemir/dstack/tree/main/skills/climb),
a separate repository whose access is managed independently. This repository
preserves the earlier contract/build implementation; it does not provide the
new climb skill.

## Portability

The build, verification, and grading code uses locally installed agent tools.
The historical planning workflow additionally requires a separately configured
`rolly agent` / `rolly linear` runner; these integrations are not bundled here.
Set `MIL_AGENT_USER` to the runner identity (otherwise the local `USER` is used).
Configure credentials locally with `mil setup`; no credentials or private
product context are distributed. Private benchmark banks and historical run
results are omitted. Supply your own bank for benchmark commands.

> **North star: peak quality on every task, made affordable by distributing heavy work across cheap
> models under a tight contract/workflow split.**
> Almost everything needs a heavier touch than an agent naively picks — so Milhouse defaults to thorough,
> never light. But thoroughness need not be expensive: Wayfinder produces one complete contract, then Codex
> runs against it on the flat-rate ChatGPT subscription —
> extensive verification, multiple passes, real rigor — while spending expensive models on judgment. The agent
> decides *how* to be thorough for each context; it never decides to skip thoroughness. We hard-code no
> domain pipeline of steps or gates — the rigor lives in the contract and workflow judgment, distributed cheaply,
> held at peak. The harness owns only what correctness demands (locks, the exit contract, the independent
> grader, the ledger); everything else is the workflow's call.

**A general autonomous agent harness.** Give it one well-specified task and it runs end-to-end.
`mil plan` produces `contract.md`, then a Codex loop runs against that frozen contract. Codex
does the work, the CLI validates `verdict.json`, and failed verifier output resumes Codex until the verdict passes or the loop cap is hit. It saves documentation of what it
did. An **independent grader** (blind model judges) then scores the run against the criteria. As
flexible as a chat with a top model; deterministic only where correctness demands it (locks, the exit
contract, the grader, the ledger).

`skills/mil-plan/SKILL.md` is the one planning workflow for every entry point. `mil plan`
loads it directly, while `mil install` links that same directory into Claude Code, Codex, and
the shared agent-skill directory. Invoke `mil-plan` inside an existing chat when you want that
conversation to remain the coordinator; use `mil plan` only when you want the CLI to open a new
coordinator conversation. There is no separate harness-specific planning prompt.

It is not a Linear tool or an iOS tool. If a prompt names a repo, Milhouse follows that repo's declared
build/test rules; domain knowledge lives in the prompt, repo, and model, never in the harness. Linear
tickets and an iOS `xcodebuild` gate are used below only as example task sources and example gates.

**Architecture (the stable concepts): [`docs/spec.md`](docs/spec.md).** Read that first. This README is
the quickstart.

```bash
mil setup --anthropic-key-file /path/to/env      # optional key for explicit Claude API profiles
mil plan --workdir /path/to/repo "shape this product change" # interactive Wayfinder session
mil plan --autonomous --issue TASK-123 --workdir /path/to/repo "fix this small issue" # logged, fail-closed contract attempt
mil contract validate --contract contract.md       # deterministic Ready-for-Build gate
mil contract render --contract contract.md         # generated collapsible contract.html
mil build --workdir /path/to/repo --contract contract.md --id task-123
mil models                                      # supported profiles and phases
mil config --plan opus-5-5-high-subscription --autoplan-review opus-5-5-high-subscription
mil cancel task-123
mil bench all --bank my-bank                     # supply bench/my-bank first
mil sanity
mil doctor --json
mil runs status
mil runs maintain --pressure warning
```

Pass `--workdir` for the directory the workflow should start in. If the task needs another specific
repo or path, put that in the prompt. A common workspace root is also valid; verification discovers
the nested Git repositories and reports their diffs together.

`mil plan` is interactive: one coordinator conversation relays between the user and isolated
Wayfinder thinkers until the map is clear, then writes `contract.md`. For a genuinely small,
unambiguous Linear issue, `--autonomous --issue` requires independent logged read-only planner and
reviewer approval before writing the same complete contract; otherwise it returns `needs_user`.
`mil build` validates, freezes, and executes that file.
Use `mil config` for saved plan/autoplan-review/build/verify assignments or `--main`/`--model` for one invocation.
Agent Bridge owns the catalog; `mil models` and `agent-bridge agents` return the same agents and ratings.
It also owns portable harness capabilities and their setup hints; `agent-bridge capabilities` shows what this installation can use, while Milhouse only selects from that output.
Planning and routine grading default to Opus 5 through the local Claude subscription. Unattended planning rejects every other profile instead of selecting or resuming an API fallback. Fable profiles remain available only when a human explicitly invokes one manually.

Autonomous builds use the configured Codex build profile in a persisted goal for each outer-loop attempt,
followed by the configured fresh read-only verifier profile. A failed review resumes the same builder
thread with a new goal and the prior feedback.

Cancel a stale or active run with `mil cancel <run-id-or-dir>`.

Each run is capped at 1 GiB. `mil runs maintain` removes only build caches from terminal runs older than seven days; run evidence and active runs are retained. A directory counts as a build cache when it carries the layout the owning tool writes — an Xcode DerivedData root, a SwiftPM workspace or bare mirror set, a compiler module cache, or a tool-owned name such as `node_modules` — never because it is merely named like one. `mil runs status` lists the same directories as `candidates` without removing anything.

Exit codes come only from the parsed result: `0` green · `2` failed · `3` blocked. Every run appends to
`~/.milhouse/runs.jsonl` and reports status + cost back to the task source.

## The loops

Milhouse owns planning, building, review, grading, and run state. Product task selection and scheduling
belong to the caller. The dashboard is only an
interface over Milhouse's CLI, JSON artifacts, and `~/.milhouse/config.json`.

## Grade — the quality gate

`mil grade` runs three **blind** judges in parallel, each a distinct lens — contract-correctness,
principles/minimality, evidence-honesty — median-aggregated, arbiter on wide spread. It is the
domain-agnostic quality bar and it must pass calibration before its scores are trusted across the trend.
Manual Fable grading uses the API key and counts as marginal cost; routine Opus grading uses the local Claude Max
subscription and records zero marginal cost.
Deterministic quality/honesty guards are deliberately avoided: they false-fail work the grade judges
better. Determinism is for actions and accounting; judgment is for the model.

## Ethos

- Minimal and readable at a glance; prompts short and model-agnostic — no prompt debt.
- Model-in-the-loop for judgment; deterministic only for actions, accounting, and lifecycle.
- Every change must simplify or add verification power — nothing else.
