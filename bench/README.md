# Benchmarks

The reusable benchmark runner accepts user-supplied task banks. Private product
fixtures, reference implementations, screenshots, and historical results are
omitted from this source distribution. Synthetic grader calibration fixtures
remain under `calibration/`.

Create `bench/<bank>/<task>/` with `card.json`, `setup.sh <destination>`,
`rubric.md`, and a `reference/` artifact directory. Then run:

```bash
mil bench all --bank <bank>
mil bench ref all --bank <bank>
mil bench calibrate
```

New scores append to local `parity/scores.jsonl`, which is ignored by Git.
