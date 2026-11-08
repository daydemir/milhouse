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

## Design decisions

- Default malformed-row policy is `fail`: failing fast is safer because a silent default skip can
  hide data loss. Use `--on-error skip` when partial salvage is intentional.
- Route proximity is the minimum distance from the beat to any route segment, with endpoints
  clamped, using a local equirectangular meter projection per segment.
- Ties sort by `(title, lat, lng)` ascending. Dedupe is greedy first-wins in CLI/file order, so the
  first same-title beat within 25 m keeps all of its fields unchanged.
