# Coordinator and thinker workers

The interactive Mil Plan agent is a coordinator, not a planner. It owns the user conversation and mechanical state transitions. Every judgment call belongs to a read-only thinker worker launched through `rolly agent` with the configured Plan profile.

## Roles

- **Coordinator:** refresh the map, claim tickets, launch or resume workers, queue their questions, relay text verbatim, apply exact worker resolutions through `rolly linear`, and enforce the contract gate. Do not answer planning questions, synthesize decisions, or improve worker wording.
- **Mapper:** one worker session that first passes [pursuit-evidence.md](pursuit-evidence.md). Only then, when pursuit is justified, it interviews for the destination and frontier and returns the exact proposed map, tickets, fog, scope, and dependency edges. It never mutates Linear.
- **Ticket thinker:** one resumable worker session per child issue. It may ask the human one question at a time or return a resolution. It never mutates Linear or delegates another thinker. A prototype thinker may write only to its disposable artifact directory.
- **Research worker:** one AFK read-only worker per research ticket. It returns sourced findings without waiting for the human.

The coordinator and workers may use the same model. Their separate roles and sessions are the boundary.

Unattended and default planning always use `opus-5-5-high-subscription`. If it is unavailable, stop with the exact error; never select, resume, or fall back to a Fable or API profile. Fable profiles remain available only for a human's explicit manual invocation.

After loading the map, use the harness's native rename capability when available to name the coordinator task `Plan · <issue> · <3–6 word outcome>`, capped at 60 characters. This is display metadata only; never pause planning when the harness cannot rename itself.

## Durable worker protocol

Write each worker turn to a prompt file and invoke:

```bash
rolly agent run --agent <plan-profile> --mode read --caller <claude|codex> --user <user> \
  --workdir <workdir> --prompt <prompt-file> --issue <ticket> --timeout 14400
```

The four-hour wall-clock limit is a stuck-worker guard, not a target duration. Override it explicitly when a known task needs longer; never shorten or abandon a sound investigation merely to fit the default.

Continue an Opus thinker only with `rolly agent resume --session <session>`, the same agent and issue, and a fresh prompt file containing the user's answer verbatim. Never resume a historical Fable or API worker in unattended planning; start a fresh Opus subscription worker instead. Passing the issue makes Rolly inject the active ticket in full plus a lean map and sibling index. The complete map remains in the run artifact for coordinator recovery; workers fetch other ticket detail only when needed.

Give the initial mapper the original task and all supplied goal context. Give later workers only the goal pointers relevant to their ticket. Never make the coordinator judge alignment itself.

Prototype tickets are the one write-mode exception. Before launch, inspect `rolly agent capabilities` and `rolly agent agents`:

- If `claude-design` is available, select the cheapest supported Claude agent with taste at least 8, preferring subscription over API. Create `~/.rolly/planning/<issue>/prototype`, launch that agent with the scratch directory as `--workdir`, `--mode write`, and `--capability claude-design`, and require `mil-prototype` plus the Claude Design MCP. Pass the product and design-system paths as read-only references.
- If Claude Design is unavailable or no qualifying Claude agent exists, use the default Plan worker with `--mode write --capability impeccable` when Impeccable is available. If neither path is available, or a selected engine fails after launch, return the ticket blocked and ask the human what to do. Never downgrade silently after an engine failure.

Resume the prototype with the same agent and session. All other thinkers use the default Plan profile in `--mode read` from the product workdir.

At coordinator startup, inspect `rolly agent capabilities` once. Every mapper, research, and prototype prompt must tell the thinker not to stay terminal-only: actively consider Browser and Computer Use whenever rendered, visual, authenticated, or multi-app evidence could materially reduce the fog. For Codex workers, pass `--capability browser` or `--capability computer-use` when selected. Prefer the most direct trustworthy surface—structured connector or CLI, then web search for public facts, Browser for rendered or authenticated websites, and Computer Use for desktop GUIs or multi-app workflows. Do not substitute Computer Use for a structured source merely because it is available.

When Browser or Computer Use is callable in the coordinator's native harness, use it there for bounded UI evidence collection; do not relay native UI through `rolly agent`. Otherwise delegate the UI task to a logged Codex worker with the selected Agent Bridge capability. Announce delegated work only after launch, and give one concrete status after ten minutes without a result or human question.

If a needed capability is unavailable, preserve Agent Bridge's exact setup hint in the blocked worker result so the coordinator can ask the human to enable it. Never duplicate capability installation or permission rules in Milhouse.

If useful evidence requires a login, new account, paid access, terms acceptance, CAPTCHA, or broader app permission, the worker must report the exact unmet access instead of acting for the human. The coordinator creates or updates a `wayfinder:task`, asks the user for the narrow action, and continues every other actionable ticket. Request access only when it would materially improve an in-scope decision, not for vague inspiration.

Start every thinker prompt with `Role: thinker worker. Do not coordinate, mutate Linear, write contract.md, or delegate another thinker.` Require exactly one JSON object, without a code fence, in one of these shapes:

```json
{"state":"question","issue":"TASK-123","title":"Decision name","question":"One question for the human","unblocks":2}
{"state":"resolved","issue":"TASK-123","title":"Decision name","answer_markdown":"Exact resolution comment","gist":"One-line map index entry","map_changes":"Exact new tickets, relations, fog, or scope edits; empty when none"}
{"state":"blocked","issue":"TASK-123","title":"Decision name","reason":"Exact unmet dependency"}
```

The mapper may instead return `state: "mapped"` with the exact parent body, child questions and types, and dependency edges. After the human agrees not to pursue now, it may return `state: "not_pursued"` with `outcome: "cancel"` or `outcome: "defer"`, the exact rationale, any concrete revisit condition, and the exact Linear update. The final contract worker may return `state: "contract"` with the complete contract text.

Questions live at the product and trade-off level — "push to production now, or version the API first?" — never at implementation-detail level ("name it X or Y?"). Research and risk discovery do not become requirements; they only resolve decisions tied to the destination. After every answered question, reorient to the fog before asking the next: where is the frontier, what do we still not understand.

Format every human-facing `question` value as readable Markdown with this exact shape:

```markdown
<One short question sentence>

1. **<Top-level idea of the option>** — <unbolded detail and tradeoff>
2. **<Top-level idea of the option>** — <unbolded detail and tradeoff>
```

Use two to four non-exhaustive options, one per line: bolded top-level idea, then detail. Mark one `(Recommended)` only with a real preference and reason — an uncertain recommendation is worse than none. No boilerplate reminders that other answers are possible. Never put context, the question, and multiple options into one paragraph or bullet. A prototype reaction question names the artifact URL once and asks exactly one reaction; the inventory belongs in the prototype's README or ticket.

When an answer defers concrete work beyond this contract, `map_changes` must name the proposed independent Backlog issue and the context it should preserve. Do not create follow-up work for ideas the human rejected.

`rolly agent runs` plus each run's `run.json`, `linear-map.json`, `result.md`, and session ID are the queue. Do not create another queue store. After restart or compaction, rebuild pending work from Linear assignments and the latest run for each issue.

## Coordinator loop

1. Refresh `rolly linear map <parent>` and `rolly agent runs`.
2. Apply any completed worker result that Linear does not yet contain.
3. Resume every open claimed ticket from its latest worker run; claim and launch a thinker for every unclaimed frontier ticket. Launch all research immediately. Launch prototype thinkers only when their blocking decisions are closed, applying the capability routing above, then link the local prototype and its source from the ticket and relay the reaction question like any other HITL ticket.
4. Queue returned questions. Before queueing, validate the payload: JSON fields (`issue`, `title`, integer `unblocks`) and the markdown shape above; bounce a malformed payload back to its worker instead of relaying it. Before asking or repeating one, consume any human answer already delivered into the current coordinator turn; never repeat an answered question. Ask only one human question at a time, choosing the one with the largest `unblocks`; break ties by issue number.
5. Relay the user's answer verbatim to that worker with `rolly agent resume` in the background, then return to the queue immediately. Never wait for that worker while another queued question or frontier ticket is actionable.
6. On `resolved`, apply the exact comment, state, map gist, new tickets, independent Backlog deferrals, relations, and fog changes; then refresh the map before using another queued question.
7. If a queued worker's saved `linear-map.json` differs from the current map after another decision, revalidate cheaply: resume the worker only when the resolved sibling blocks, or is named in, the saved question; otherwise relay the saved question unchanged. Sending an answer alone does not stale other questions.
8. When a worker returns `blocked` on an external wait with no human action available (a third-party review, a report still generating), convert it to a `wayfinder:task` ticket carrying the recheck condition and stop relaunching that worker. Record the source SHA a contract targets so critics flag substrate drift once, not per round.
9. Continue until the contract gate passes, the human accepts a documented `not_pursued` outcome, the user exits, or every worker is blocked. Never wait for one worker while another frontier ticket or queued question is actionable.

For final synthesis, launch a contract worker from the completed map with [contract-format.md](contract-format.md): it distills the resolutions into the minimum sufficient contract — a deletion pass is part of synthesis, and completed ticket prose is input to distill, not settled scope to accumulate. The writer and fresh critic both reapply [pursuit-evidence.md](pursuit-evidence.md) if the synthesis would preserve, version, or rebuild a named concept whose current-use basis is missing or contradicted by later evidence. Route critique back to the contract worker until it returns a passing contract. The coordinator copies that exact contract into `contract.md`; it does not rewrite it, then runs the contract gate exactly as the Milhouse adapter specifies.
