---
name: mil-prototype
description: Build the cheapest disposable artifact that makes a Wayfinder prototype ticket concrete enough for a human decision. Use only during Mil Plan for a wayfinder:prototype ticket; never as production implementation.
---

# Mil Prototype

Turn one fuzzy design or behavior question into something the user can react to. The artifact is evidence for a planning decision, not part of the product.

## Boundaries

- Work only in the scratch directory the coordinator provides, normally `~/.rolly/planning/<issue>/prototype`.
- Read the product repo, its `AGENTS.md`, design system, and existing UI for grounding. Never edit the product repo.
- For UI or other shape questions, default to three materially different options and cap at five. They must disagree about structure, hierarchy, flow, or primary affordance—not merely color or copy.
- For logic or state questions, compare plausible models when the decision has real alternatives and exercise the same hard cases against each. If only one direction is feasible, explain why in `README.md` instead of inventing variants.
- Use real domain language and representative data, clearly marked as sample data.
- Prefer a dependency-free standalone `index.html` for visual or interactive work. Use a short Markdown sketch for nonvisual flows. Use isolated native code only when platform behavior itself is the decision.
- Do not add production architecture, analytics, persistence, auth, or other implementation hidden behind the prototype.

## Visual work

When the coordinator requires Claude Design, use its MCP as the prototype engine and export the result plus its source into the scratch directory. A hosted canvas or image without local source is incomplete. If the MCP is unavailable or fails, return `state: "blocked"`; do not hand-code a substitute.

Otherwise, use Impeccable's `shape` and `craft` guidance when installed; use `frontend-design` only when Impeccable is unavailable. Ground the result in the repo's actual design system before taking a new aesthetic direction. Match the fidelity to the question: information hierarchy and behavior matter more than decorative polish in a planning prototype.

The standalone HTML must work by opening it directly, without a build or network request. Put every option in one artifact with a visible switcher; make the selected option shareable through a URL search parameter when practical. Respect keyboard focus, reduced motion, and a useful narrow layout.

## Deliverable

Create:

- the locally viewable prototype artifact;
- all code and source files needed to render it;
- `README.md` with the ticket question, artifact and source paths, how to open it, what is real versus simulated, each option's name and material tradeoff, and the decision it is meant to elicit.

Inspect the artifact locally when tools permit. Then return exactly one worker JSON object using the Mil Plan protocol:

- `state: "question"` with the artifact path and one high-leverage comparison question when human judgment is still needed;
- `state: "resolved"` only after the human has answered and the prototype has captured the settled direction.

The coordinator links both the viewable artifact and its source from the ticket, then relays the question. It does not interpret the prototype.
