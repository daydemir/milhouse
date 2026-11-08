# Contract format

`contract.md` is the acceptance contract an autonomous builder executes and ships to production sight unseen. It is **minimum sufficient**: exactly what recognizes the requested outcome and prevents material regression — no more. Done means live in production with no human gate, so everything in it must be completable by a builder in a loop, alone.

## The thesis tree

The whole contract is one grammar, at four altitudes:

- **L1 — thesis.** One full sentence, bold. Drops decoration, never the subject or the verb; it must survive out of context. Reading only the L1s gives the whole contract.
- **L2 — detail.** One or two sentences under the thesis.
- **L3 — proof and expect** (acceptance criteria only): the command the verifier runs and the pass condition.
- **L4 — builder detail.** A nested block, collapsed in HTML: enumerations, fixtures, environment facts, what the verifier must assert. Precision lives here, in the node it belongs to — never in a parallel list.

No plan self-commentary anywhere: not how the contract got here, what was cut, or how minimal it is. Say why the plan is the way it is.

## Sections

Exact H2 order:

1. `Why` — one to three thesis nodes: the problem, and why now.
2. `Decisions` — one node per load-bearing decision, linking its Wayfinder ticket or evidence.
3. `Boundaries` — two nodes: what the builder owns, and what is not in this build.
4. `Shipping` — the builder-executable route to production: how it goes live (flag or staged where risk warrants), where proofs run (disposable sandboxes, never real channels), and a rollback the builder can trigger itself.
5. `Acceptance` — the criteria (below).

## Acceptance criteria

Each criterion is a node: id (`A1`…`An`), kind, L1 thesis, optional L2, `proof:` command, `expect:` pass condition, optional L4.

- Kinds: **meet** (an outcome or invariant to achieve), **never** (a hard constraint), **hold** (must not regress).
- Every criterion answers: *what concrete failure of the requested outcome would this catch?* Weak, speculative, duplicated, or ceremonial answers mean delete it.
- Quantify only where a number is the real bar; otherwise be function-driven over enumerated sets ("for each surface in the Surfaces list, capture writes a record…"). The enumerated set lives in the contract once and is referenced.
- No human approvals, elapsed-time observation, or genuine-future-traffic in any criterion. Irreducible external gates (an app-store review, a third-party wait) are prerequisites recorded on the issue, outside builder acceptance.
- No count cap. The test is honesty, not arithmetic: a large feature may need many criteria across platforms, but each must cover something conceptually important — no redundancy, no dirt. Builder-reasoning test: if guaranteeing X makes the builder messing up Y near-impossible, omit Y.
- One strongest practical proof per outcome. Provenance and evidence, not proof-spinning: no matrices of every conceivable fixture, no hundreds of UI tests, no test debt.

The verifier checklist is a **mechanical projection** of these nodes — id, kind, proof, expect — never a separately authored list. What the human approved at L1 is exactly what the verifier enforces.

## Critic

The critic's goal is the contract's goal: **scope and precision**, judged by thinking about what the builder will actually do. First move is delete, merge, or relax. It may add or block only for something conceptually important the criteria miss, a material regression or data-loss/security/privacy hole, or a core outcome the verifier cannot check. No wargaming unknown unknowns for its own sake; no speculative assurance work. When 100% of the contract passes, the original intent is satisfied without another planning pass.

## Markdown mapping

`contract.md` renders deterministically to `contract.html` via `mil contract render`; never edit or independently author the HTML.

- H2 sections as above; the document opens with a one-sentence H1 thesis for the whole contract.
- A node is a list item: `**<thesis sentence.>** <detail>`.
- A criterion uses this exact Markdown shape (the list markers are required):

  ```markdown
  - A1 (meet): **The observable outcome works.**
    - proof: command that verifies the outcome
    - expect: exit 0 and the named observable result
  ```

  Continue with `A2`, `A3`, and so on; replace `meet` with `never` or `hold` when appropriate.
- L4 is a nested list under a `builder detail:` line; it renders collapsed.
