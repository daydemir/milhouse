# Agent selection

`agent-bridge agents` is the single source for every supported agent's model, authentication, effort, intelligence, taste, cost, and intended use. Do not copy that catalog into Milhouse or project instructions.

## Connection rules

- Milhouse resolves selected agent IDs through `agent-bridge agents` and fails if the dependency or ID is invalid.
- Agent Bridge owns model, authentication, effort, and ranking changes; Milhouse owns only Plan, Build, and Verify assignments.
- Never print credentials. Use existing login/key stores.
- `mil plan` may use any catalog agent. Build and Verify require Codex subscription agents because those phases depend on persisted Codex execution and read-only review semantics.

Run `mil models` or `agent-bridge agents` to inspect the same catalog. Run `mil config` to see current assignments, or save them with `mil config --plan <agent> --build <agent> --verify <agent>`. Per-invocation `mil plan --main` and `mil build --model` override the saved Plan and Build assignments.
