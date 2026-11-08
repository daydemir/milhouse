# `beatmerge` — merge geotagged story-beats from heterogeneous sources

Build a small Python CLI, `beatmerge.py`, that merges geotagged **story-beat** records drawn
from three heterogeneous source files, dedupes near-duplicates, orders the survivors by their
proximity to a walking route, and emits typed JSON on stdout.

This is a synthetic audio-tour problem: story beats are points a walker hears audio at. The three sources
are the same beats exported by three different tools, so formats and field names diverge and
some rows are junk.

## CLI contract (fixed — `check.py` depends on it)

```
python3 beatmerge.py --sources FILE [FILE ...] --route "LAT,LNG;LAT,LNG;..." \
                     [--on-error {skip,fail}]
```

- `--sources` — one or more source files. Format is inferred from the extension:
  - `.json`  → a JSON **array** of beat objects.
  - `.csv`   → header row then data rows.
  - `.jsonl` → one JSON beat object per line.
- `--route` — an ordered walking path as `;`-separated `lat,lng` vertices.
- `--on-error` — malformed-row policy. `check.py` exercises **both** values explicitly.
- Output: a JSON **array** printed to stdout, one object per surviving beat, in route order.

### Beat fields (post-normalization, typed)

Every emitted object MUST have at least:

| field   | type    | notes                                   |
|---------|---------|-----------------------------------------|
| `title` | string  | non-empty                               |
| `lat`   | number  | finite float                            |
| `lng`   | number  | finite float                            |

Source rows may name these differently (e.g. `name`/`latitude`/`longitude`, or CSV columns
`title,lat,lng,note`). Normalize them. You may carry extra fields through; the three above are
the contract.

## What "merge" means

1. **Load** all rows from all sources into one normalized stream.
2. **Dedupe near-duplicates**: two beats with the **same title** whose points are within **25 m**
   of each other are the same beat — collapse to one. Same title but > 25 m apart are **distinct**
   beats and both survive. (Title alone is NOT the key; geography matters.)
3. **Order** survivors by proximity to the `--route` path, nearest-first. The beat sitting on the
   route's start should come out first.
4. **Emit** the typed JSON array.

## Deliberately underspecified — you decide, and DOCUMENT your call

These are design surfaces. A reasonable, clearly-documented resolution scores well; silence or an
arbitrary undocumented choice scores poorly.

- **Default malformed-row policy** (when `--on-error` is omitted): should a malformed row make the
  whole run **fail fast**, or be **skipped** with the rest surviving? Pick one, implement it as the
  default, and justify it in a comment or the README. (`check.py` only ever calls the explicit
  `skip` / `fail` modes — the *default* is yours.)
- **Proximity-to-a-path metric**: "distance to the route" for a multi-vertex path is ambiguous
  (nearest vertex? nearest point on any segment? arc-length projection?). Choose a sensible metric
  and state it.
- **Tie-break**: when two survivors are equidistant from the route — or when two near-duplicates
  disagree on coordinates/extra fields and you must keep one — the ordering / which-to-keep rule is
  unspecified. Make it deterministic and say what it is.

## Constraints

- **Fail fast** on genuinely broken input under `--on-error fail`: nonzero exit, no partial JSON on
  stdout.
- **Performance**: merging **10,000 rows** must complete in **under 2 seconds** on a laptop.
- Typed, self-documenting code. No loose magic strings for the source-format or error-policy
  choices — use enums / typed constants. Minimize global mutable state.
- Standard library only.

## Definition of Done (quantified)

- `python3 check.py` prints `PASS` and exits 0. (~12 acceptance checks, in the repo — read them.)
- Your own unit tests (`python3 -m unittest discover -q`) are green.
- The three underspecified decisions above are documented.
