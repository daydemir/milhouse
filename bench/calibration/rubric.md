# Calibration rubric — `beatmerge` calibration (scorer anchor)

This rubric exists to CALIBRATE the bench scorer, not to grade new work. Two frozen implementations
of the `beatmerge` calibration task (full spec in each dir's `task.md`) have known reference scores from
prior real parity scoring: `known-67` ≈ **67**, `known-86` ≈ **86**. A healthy scorer reproduces each
within ±5. Score the given implementation 0–100 on overall quality-vs-ideal and cite evidence.

## What the task demanded (summary)

A `beatmerge.py` CLI that merges geotagged story-beats from `.json`/`.csv`/`.jsonl`, dedupes
same-title beats within 25 m, orders survivors by proximity to a `--route` polyline, emits typed
JSON. Three underspecified surfaces (default `--on-error` policy, the path-proximity metric, the
tie-break/which-to-keep rule) must be RESOLVED and DOCUMENTED. Constraints: fail-fast on broken
input, 10k rows < 2s, typed/enum'd (no loose format/policy strings), stdlib only. DoD: `check.py`
prints PASS, own unit tests green, the three decisions documented.

## Dimensions

| Weight | Dimension | Notes |
|-------:|-----------|-------|
| 30 | Correctness | `check.py` PASS + own tests green = the functional floor (both anchors clear this) |
| 25 | Decision documentation | are the three underspecified surfaces resolved AND clearly justified? depth matters |
| 20 | Test thoroughness | breadth/edge coverage of the author's own suite, not just the acceptance checks |
| 15 | Code quality | typed, enum'd choices, no loose strings, minimal global state, readable |
| 10 | Edge-case rigor | explicit per-row vs whole-input error policy, transitive-dedupe/tie-break reasoning |

## Anchoring bands (calibrated to the two references — apply literally)

* **~85–88 (the `known-86` shape):** correct AND thorough. A rich own test suite (roughly **20+**
  tests), an explicit written policy for edge cases (per-row error handling, transitive dedupe chains,
  a stated total sort key), and decisions justified with reasoning — not just asserted. This is the
  "indistinguishable from a careful engineer" band.
* **~65–69 (the `known-67` shape):** correct and passing, but THINNER. A modest own test suite
  (roughly **~13** tests), decisions documented but briefly, and little articulation of the harder
  edge cases (transitive chains, per-row vs whole-input policy). Solid and shippable, not exhaustive.
* Below ~60: fails `check.py`, undocumented decisions, or loose/untyped choice handling.
* Above ~90: reserved for correctness PLUS exhaustive tests PLUS airtight edge-case docs PLUS notably
  clean design — beyond either anchor.

Judge on observable depth: count the author's tests, read how thoroughly the three decisions and the
edge cases are written up. Thin-but-correct lands ~67; thorough-and-correct lands ~86.
