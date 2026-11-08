# beatmerge

A CLI that merges geotagged story-beats from three heterogeneous sources (json/csv/jsonl),
dedupes near-duplicates, orders by walking-route proximity, and emits typed JSON.

**The spec is [`task.md`](./task.md).** Read it — it holds the CLI contract, the merge rules,
three deliberately-underspecified design decisions you must resolve and document, and the
quantified Definition of Done.

## Verify

```
python3 check.py                       # prints PASS when done (~12 acceptance checks)
python3 -m unittest discover -q        # your unit tests
```

Fresh checkout: `beatmerge.py` is an empty stub, so `check.py` fails. That is the baseline.

## Design decisions

The three deliberately-underspecified surfaces in `task.md`, resolved:

1. **Default `--on-error` is `fail`** (fail fast). Silently dropping beats corrupts a walk;
   lossy behavior must be opted into explicitly with `--on-error skip`.
2. **Route-proximity metric: nearest point on any route segment** — great-circle distance from
   the beat to the closest point of the polyline, segment endpoints clamped. Computed by
   projecting into a local equirectangular meter frame anchored at each segment (exact enough
   at walking scale, and strictly better than nearest-vertex for beats abeam a long segment).
3. **Determinism.** Near-dup collapse is **first-seen wins**: sources in CLI order, rows in
   file order; the winner's full record (coords + extras) is kept unmerged. The collapse is
   *greedy*: each row survives iff it is > 25 m from every already-kept same-title beat, so
   for transitive chains (A–B ≤ 25 m, B–C ≤ 25 m, A–C > 25 m) the surviving *set* — not just
   which record wins — follows encounter order. That order is fixed (CLI source order, then
   file order), so any given invocation is fully deterministic. Output ordering uses the
   total sort key `(route_distance_m, title, lat, lng)`.

Edge-case policy: the `--on-error` policy applies **per row** (missing/empty title, missing or
non-numeric/non-finite coordinates, unparseable JSONL line, a CSV row with more cells than the
header). Broken *input* — unreadable file,
invalid or non-array top-level JSON, a CSV header missing the title/lat/lng columns, an unknown
extension — is always fatal (exit 1, no JSON array on stdout), regardless of policy. The 25 m
dedupe radius is inclusive (`dist <= 25 m` collapses).
