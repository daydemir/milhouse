@caveman ultra for non-artifact output. Keep code, JSON, commands, paths, errors, and required evidence complete and exact.

Read the embedded run.json, contract.md, and run-history.md sections below. Artifact files live under the run_dir in run.json if deeper inspection is needed.

Do the work needed to satisfy contract.md 100%.

At the start of this attempt, create a persisted Codex goal for satisfying this contract and the latest verifier feedback. Keep the goal objective under 4,000 characters and put detail in the supplied files. Work autonomously until the goal is terminal. Mark it complete only after the required checks and durable evidence pass. If this resumed thread contains a prior terminal goal, create a new goal for this attempt.

If credentials are needed, use the task prompt or target repo's credential instructions. Never print secrets.

If you believe you are stuck and cannot make further progress toward satisfying contract.md, do this only after making a concrete fix attempt: append a `Codex @ATTEMPT@` section to `run-history.md` with what you tried, what is still failing, why you believe you are stuck, and a suggestion for the next agent to try. Then stop.

contract.md is the contract. Do not optimize for anything outside contract.md, and do not shortchange any part of contract.md.

Rules:
- Preserve completed work unless it conflicts with contract.md.
- Leave durable evidence under `@RUN_DIR@/evidence/`, including `@RUN_DIR@/evidence/index.md` with paths and key command outputs.
- A check likely to take over 10 minutes must survive this worker. Launch or adopt it with `~/.milhouse/bin/durable-command start --state @RUN_DIR@/evidence/jobs/<name> -- <command>` and poll with `~/.milhouse/bin/durable-command status --state ...`. Never restart a running or completed job; later attempts must adopt its log and result.
- Append a brief `Codex @ATTEMPT@` section to `@RUN_DIR@/run-history.md`: what changed, checks run, evidence paths.
- Do not edit `contract.md`, `run.json`, or prior history except appending your `Codex @ATTEMPT@` section.
- Do not write `verdict.json`. Milhouse owns it: green after verifier agent passes, `hit_run_limit` after the attempt cap.
- Use Codex subagents as needed, both in parallel and sequentially and to help you improve your work.
- Apply DRY, functional, lean, clean, simple AF changes. Use hard cuts, fail fast with clear errors, and type-safe enums for choices, discriminated unions for explicitness. Contracts in code. Less code is always better.
- Minimize global mutable state, side effects, nil checks, force unwraps, and loose strings.
- CLI-driven wherever possible: build functionality in the CLI/core, prove it end to end with CLI checks, then UI on top.
- Prefer the smallest complete solution that satisfies `contract.md`; do not add abstractions, configuration, fallback paths, or scaffolding for future needs unless the contract explicitly requires them.

run.json:
@RUN_JSON@

contract.md:
@CONTRACT_MD@

run-history.md:
@RUN_HISTORY@
