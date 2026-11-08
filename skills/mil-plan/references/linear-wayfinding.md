# Linear wayfinding operations

Use only `rolly linear`; never mutate Linear directly. Resolve current IDs with `rolly linear statuses` and `rolly linear labels` instead of hardcoding them.

## Pursuit gate

Before applying `wayfinder:map`, moving the parent to `Planning`, or creating children, test whether the issue should be pursued now. If the human accepts cancellation, comment with the goal-based rationale and evidence, then move the parent to `Canceled`. If the human accepts deferral, comment with the rationale and a concrete revisit condition, then move or leave the parent in `Backlog`. Do not create a duplicate follow-up issue or `contract.md`; this is a successful planning outcome.

If that conclusion emerges after a map exists, stop its workers, cancel every open child, preserve completed decisions for context, and move the parent as above.

## Chart

- The planning parent is the Wayfinder map. Move it to `Planning`, apply `wayfinder:map`, and replace its description with the upstream map-body template through `rolly linear description <issue>` on stdin.
- Create each sharp decision as a child with `rolly linear subissue <parent> <title> <planning-state-id> <type-label-id>`, passing only its `## Question` body on stdin.
- Subscribe the requester to the parent map only. Do not subscribe people to its decision tickets.
- Create all tickets first. Then add each native dependency with `rolly linear blocks <blocker> <blocked>`.
- Use exactly one type label per ticket: `wayfinder:research`, `wayfinder:prototype`, `wayfinder:grilling`, or `wayfinder:task`.
- Delegate every research ticket per [orchestration.md](orchestration.md). Put the returned artifact path and resolution gist in a ticket comment, close it, then update the map index.

## Resume and frontier

Run `rolly linear map <parent-or-child>`. It ascends to the map root and returns the entire child tree, comments, claims, labels, and native relations.

A frontier ticket is a child whose state is neither completed nor canceled, whose assignee and delegate are empty, and whose open blockers are empty. An open blocker appears in `inverseRelations.nodes` with type `blocks` and an `issue.state.type` that is neither completed nor canceled.

Claim each chosen ticket first with `rolly linear assign-me <ticket>`. Each thinker works at most one non-research ticket; the coordinator may manage every claimed thinker in one conversation.

## Resolve

1. Post the answer with `rolly linear comment <ticket>` using stdin for long text.
2. Move the ticket to `Done` with `rolly linear state <ticket> <done-state-id>`.
3. Append only its linked one-line gist to `Decisions so far`, then replace the parent description with `rolly linear description <parent>`.
4. Create newly sharp tickets, add dependencies in a second pass, and remove stale dependencies with `rolly linear relation-delete <relation-id>`.

## Defer beyond this contract

When the human explicitly pushes concrete work beyond the current destination, preserve it as a normal independent Backlog issue. This is for deferred work, not ideas the human rejected.

1. Search for an existing matching issue and reuse it when present.
2. Resolve the current Backlog ID with `rolly linear statuses`, then create the issue with `rolly linear create <title> <backlog-state-id>`.
3. Give it a self-contained description with `rolly linear description <issue>`: link the planning parent and the exact ticket or resolution that deferred it, state why it is outside this contract, and preserve the remaining work or decision.
4. Link that Backlog issue from the map's `Out of scope` line. If the deferred item was already a Wayfinder child, close that child as upstream requires and link both records.

The Backlog issue is not a child of the map, carries no `wayfinder:*` label, and has no blocking edge to the map. It must remain discoverable without preventing the contract gate. A later planning effort may adopt it as a new map.

## Contract gate

Keep the parent in `Planning` while any child remains open or `Not yet specified` contains in-scope fog. After the Milhouse adapter's contract gate passes, upload `contract.md` and `contract.html` with `rolly linear upload` and append only a stub under `## Build contract` in the parent description — the two links plus a `source_hash: <hash>` line (the render's returned hash). Never paste contract text into an issue body: the parent stays a brief map. Autopilot verifies the hash against `~/.rolly/planning/<issue>/contract.md` and reads the contract from there. Then move the parent to `Ready for Build`. Never move decision tickets to `Ready for Build`.
