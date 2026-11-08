# Milhouse architecture

Milhouse is a reusable, local-first harness for turning a task into a verifiable autonomous run. Product-specific intake and scheduling stay outside the harness.

## Planning surfaces

- `mil plan` is interactive. A selected Claude Code or Codex main agent follows the Wayfinder-based `mil-plan` skill, may call selected cross-provider advisors through `mil advise`, and charts or resolves one decision session. It writes `contract.md` only when the map is clear. Its `--autonomous --issue` exception requires independent logged read-only planner and reviewer approval for a small, unambiguous issue and otherwise returns `needs_user`.
- `mil contract render` deterministically derives a collapsible `contract.html` review artifact from that Markdown. The HTML is never edited or used as build input.
- `mil build` validates, copies, and hash-pins `contract.md` before execution. There is no second planning or rubric phase.
- `mil models` returns the Agent Bridge catalog. `mil config --plan|--autoplan-review|--build|--verify <agent>` saves validated phase assignments; per-run flags override them.

## Build loop

1. The selected builder receives the frozen contract, run history, and repository context.
2. It creates and pursues one persisted Codex goal for the outer attempt, performs the work, records evidence, and marks the goal terminal.
3. A fresh selected verifier reviews read-only evidence and the worktree against the contract.
4. A passing review writes the green verdict. A failing review is appended to run history and the same builder thread resumes with a new goal.
5. The loop stops only on a valid pass, explicit failure/block, cancellation, or its configured limit.

## Deterministic envelope

- A single-writer lock prevents overlapping builds.
- Strict typed JSON artifacts define task, run, verifier, verdict, and result contracts.
- The contract is hash-pinned; mutation aborts the run.
- Missing prompts, context, history, evidence, or malformed artifacts fail closed.
- Run history and result data are durable under `~/.milhouse`.
- Agent Bridge binds each agent ID to its model, authentication, effort, and ratings. Milhouse resolves that catalog at runtime and never silently substitutes an agent or billing path.

## Boundaries

- Milhouse does not select Linear issues, manage launchd, or decide product workflow states. The caller owns those operations.
- The dashboard reads run/benchmark/config artifacts and exposes Milhouse controls. Embedding and authentication belong to the caller.
- Historical OneShot artifacts remain readable; new artifacts use Milhouse names.
- The code is the source of truth for literal commands, timeouts, and schemas.
