#!/usr/bin/env python3
"""beatmerge — merge geotagged story-beats from heterogeneous sources.

Pipeline: parse args -> load sources -> normalize rows -> dedupe near-duplicates
-> order by route proximity -> emit one typed JSON array on stdout.

Documented design calls (see README "Design decisions"):
- Default --on-error is FAIL (fail fast; lossy skipping is opt-in).
- Route metric is great-circle distance to the nearest point on any route segment.
- Determinism: near-dup collapse keeps the first-seen row (CLI source order, then
  file order); output sorts by (route_distance_m, title, lat, lng).
"""
from __future__ import annotations

import argparse
import csv
import json
import math
import sys
from collections.abc import Callable, Iterator, Mapping, Sequence
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Final

EARTH_RADIUS_M: Final[float] = 6_371_000.0
M_PER_DEG: Final[float] = math.radians(1.0) * EARTH_RADIUS_M
DEDUP_RADIUS_M: Final[float] = 25.0  # inclusive: dist <= 25 m collapses

_TITLE_KEYS: Final[tuple[str, ...]] = ("title", "name")
_LAT_KEYS: Final[tuple[str, ...]] = ("lat", "latitude")
_LNG_KEYS: Final[tuple[str, ...]] = ("lng", "longitude")
_ALIAS_KEYS: Final[frozenset[str]] = frozenset(_TITLE_KEYS + _LAT_KEYS + _LNG_KEYS)

Point = tuple[float, float]  # (lat, lng) degrees


class BrokenInput(Exception):
    """Broken *input* (unreadable file, bad top-level JSON, bad CSV header).

    Always fatal, regardless of --on-error: this is not a per-row problem.
    """


class MalformedRow(ValueError):
    """A single bad row; handled per the ErrorPolicy."""


class ErrorPolicy(Enum):
    """Malformed-row policy. Default is FAIL: silently dropping beats corrupts a
    walk, so lossy behavior must be requested explicitly with `skip`."""

    SKIP = "skip"
    FAIL = "fail"

    def __str__(self) -> str:  # clean argparse choices/help rendering
        return self.value


class SourceFormat(Enum):
    JSON = ".json"
    CSV = ".csv"
    JSONL = ".jsonl"

    @classmethod
    def from_path(cls, path: Path) -> "SourceFormat":
        try:
            return cls(path.suffix.lower())
        except ValueError:
            raise BrokenInput(f"{path}: unsupported extension {path.suffix!r} "
                              f"(expected one of {[f.value for f in cls]})") from None


@dataclass(frozen=True)
class Beat:
    title: str
    lat: float
    lng: float
    extra: Mapping[str, object]

    @property
    def point(self) -> Point:
        return (self.lat, self.lng)

    def record(self) -> dict[str, object]:
        return {**self.extra, "title": self.title, "lat": self.lat, "lng": self.lng}


# ---- geometry -------------------------------------------------------------------------------

def haversine_m(a: Point, b: Point) -> float:
    p1, p2 = math.radians(a[0]), math.radians(b[0])
    dp = math.radians(b[0] - a[0])
    dl = math.radians(b[1] - a[1])
    h = math.sin(dp / 2) ** 2 + math.cos(p1) * math.cos(p2) * math.sin(dl / 2) ** 2
    return 2 * EARTH_RADIUS_M * math.asin(math.sqrt(h))


def _point_to_segment_m(p: Point, a: Point, b: Point) -> float:
    """Distance from p to segment a-b, endpoints clamped.

    Computed in a local equirectangular meter frame anchored at the segment
    midpoint latitude — exact enough at walking scale (< a few km).
    """
    coslat = math.cos(math.radians((a[0] + b[0]) / 2))

    def xy(q: Point) -> tuple[float, float]:
        return ((q[1] - a[1]) * coslat * M_PER_DEG, (q[0] - a[0]) * M_PER_DEG)

    px, py = xy(p)
    bx, by = xy(b)
    ab2 = bx * bx + by * by
    t = 0.0 if ab2 == 0.0 else min(1.0, max(0.0, (px * bx + py * by) / ab2))
    return math.hypot(px - t * bx, py - t * by)


def route_distance_m(point: Point, route: Sequence[Point]) -> float:
    """Great-circle distance from point to the nearest point on the route polyline."""
    if len(route) == 1:
        return haversine_m(point, route[0])
    return min(_point_to_segment_m(point, a, b) for a, b in zip(route, route[1:]))


# ---- normalization --------------------------------------------------------------------------

def _pick(raw: Mapping[str, object], keys: Sequence[str]) -> object:
    for key in keys:
        if key in raw and raw[key] is not None:
            return raw[key]
    raise MalformedRow(f"missing {keys[0]!r}: {raw!r}")


def _as_title(value: object, raw: Mapping[str, object]) -> str:
    if not isinstance(value, str) or not value.strip():
        raise MalformedRow(f"bad title: {raw!r}")
    return value


def _as_coord(value: object, raw: Mapping[str, object]) -> float:
    if isinstance(value, bool):
        raise MalformedRow(f"bool coordinate: {raw!r}")
    if isinstance(value, str):
        try:
            value = float(value)
        except ValueError:
            raise MalformedRow(f"non-numeric coordinate: {raw!r}") from None
    if not isinstance(value, (int, float)) or not math.isfinite(value):
        raise MalformedRow(f"non-finite coordinate: {raw!r}")
    return float(value)


def normalize(raw: object) -> Beat:
    """Raw row mapping -> typed Beat. Raises MalformedRow on any bad row."""
    if not isinstance(raw, Mapping):
        raise MalformedRow(f"row is not an object: {raw!r}")
    if not all(isinstance(k, str) for k in raw):
        # csv.DictReader files cells beyond the header under a None restkey.
        raise MalformedRow(f"row has cells beyond the header: {raw!r}")
    title = _as_title(_pick(raw, _TITLE_KEYS), raw)
    lat = _as_coord(_pick(raw, _LAT_KEYS), raw)
    lng = _as_coord(_pick(raw, _LNG_KEYS), raw)
    extra = {k: v for k, v in raw.items() if k not in _ALIAS_KEYS}
    return Beat(title=title, lat=lat, lng=lng, extra=extra)


# ---- loading --------------------------------------------------------------------------------

def _rows_json(path: Path) -> Iterator[object]:
    try:
        data = json.loads(path.read_text())
    except ValueError as exc:
        raise BrokenInput(f"{path}: invalid JSON: {exc}") from None
    if not isinstance(data, list):
        raise BrokenInput(f"{path}: top-level JSON must be an array")
    yield from data


def _rows_csv(path: Path) -> Iterator[object]:
    with path.open(newline="") as fh:
        reader = csv.DictReader(fh)
        header = reader.fieldnames
        if header is None or not all(
            any(k in header for k in keys) for keys in (_TITLE_KEYS, _LAT_KEYS, _LNG_KEYS)
        ):
            raise BrokenInput(f"{path}: CSV header missing title/lat/lng columns (got {header})")
        yield from reader


def _rows_jsonl(path: Path) -> Iterator[object]:
    """Unparseable lines are *yielded* as MalformedRow values, not raised: raising
    would finalize this generator and silently drop every later line in the file."""
    with path.open() as fh:
        for lineno, line in enumerate(fh, 1):
            if not line.strip():
                continue
            try:
                yield json.loads(line)
            except ValueError:
                yield MalformedRow(f"{path}:{lineno}: unparseable JSONL line")


_ROWS_BY_FORMAT: Final[Mapping[SourceFormat, Callable[[Path], Iterator[object]]]] = {
    SourceFormat.JSON: _rows_json,
    SourceFormat.CSV: _rows_csv,
    SourceFormat.JSONL: _rows_jsonl,
}


def load_sources(paths: Sequence[Path], policy: ErrorPolicy) -> list[Beat]:
    """Load + normalize all rows, in CLI source order then file order.

    MalformedRow is skipped (with a stderr note) or fatal per `policy`;
    BrokenInput always propagates.
    """
    beats: list[Beat] = []
    for path in paths:
        if not path.is_file():
            raise BrokenInput(f"{path}: not a readable file")
        for raw in _ROWS_BY_FORMAT[SourceFormat.from_path(path)](path):
            try:
                if isinstance(raw, MalformedRow):  # bad JSONL line, yielded not raised
                    raise raw
                beats.append(normalize(raw))
            except MalformedRow as exc:
                if policy is ErrorPolicy.FAIL:
                    raise
                print(f"beatmerge: skipped: {exc}", file=sys.stderr)
    return beats


# ---- merge ----------------------------------------------------------------------------------

_CELL_DEG: Final[float] = DEDUP_RADIUS_M / M_PER_DEG  # one 25 m grid cell, in degrees of latitude
_LNG_CELLS: Final[int] = math.ceil(360.0 / _CELL_DEG)
_MAX_GRID_LAT: Final[float] = 89.9  # poleward of this, lng cells degenerate -> linear bucket


class _TitleIndex:
    """Spatial hash of the kept beats for one title.

    25 m grid cells keyed by (lat cell, wrapped lng cell); near-pole beats go to a
    linear bucket instead (lng cells degenerate there). Lookup scans the polar bucket
    plus a 3 x (2*span+1) cell neighborhood, where span = ceil(1/cos(lat)) so a 25 m
    great-circle separation can never out-range the searched lng cells."""

    __slots__ = ("_grid", "_polar")

    def __init__(self) -> None:
        self._grid: dict[tuple[int, int], list[Beat]] = {}
        self._polar: list[Beat] = []

    @staticmethod
    def _cell(point: Point) -> tuple[int, int]:
        lat, lng = point
        return (math.floor(lat / _CELL_DEG), math.floor((lng % 360.0) / _CELL_DEG))

    def has_within_radius(self, beat: Beat) -> bool:
        if any(haversine_m(beat.point, k.point) <= DEDUP_RADIUS_M for k in self._polar):
            return True
        i, j = self._cell(beat.point)
        span = math.ceil(1.0 / math.cos(math.radians(min(abs(beat.lat) + _CELL_DEG, _MAX_GRID_LAT))))
        return any(
            haversine_m(beat.point, kept.point) <= DEDUP_RADIUS_M
            for di in (-1, 0, 1)
            for dj in range(-span, span + 1)
            for kept in self._grid.get((i + di, (j + dj) % _LNG_CELLS), ())
        )

    def add(self, beat: Beat) -> None:
        if abs(beat.lat) > _MAX_GRID_LAT:
            self._polar.append(beat)
        else:
            self._grid.setdefault(self._cell(beat.point), []).append(beat)


def dedupe(beats: Sequence[Beat]) -> list[Beat]:
    """Collapse same-title beats within DEDUP_RADIUS_M (inclusive); first-seen wins,
    and the winner's full record (coords + extras) is kept unmerged. Greedy: each beat
    is compared against already-kept beats only, via a per-title spatial hash so a
    single dominant title stays ~O(n) rather than O(n^2)."""
    index_by_title: dict[str, _TitleIndex] = {}
    survivors: list[Beat] = []
    for beat in beats:
        index = index_by_title.setdefault(beat.title, _TitleIndex())
        if index.has_within_radius(beat):
            continue
        index.add(beat)
        survivors.append(beat)
    return survivors


def order_by_route(beats: Sequence[Beat], route: Sequence[Point]) -> list[Beat]:
    """Nearest-to-route first; total deterministic tie-break (title, lat, lng)."""
    return sorted(beats, key=lambda b: (route_distance_m(b.point, route), b.title, b.lat, b.lng))


# ---- CLI ------------------------------------------------------------------------------------

def parse_route(spec: str) -> tuple[Point, ...]:
    try:
        vertices = tuple(
            (float(lat), float(lng))
            for lat, lng in (pair.split(",") for pair in spec.split(";") if pair.strip())
        )
    except ValueError:
        raise argparse.ArgumentTypeError(f"bad route spec {spec!r}") from None
    if not vertices or not all(math.isfinite(c) for v in vertices for c in v):
        raise argparse.ArgumentTypeError(f"route needs finite 'lat,lng;...' vertices: {spec!r}")
    return vertices


def parse_args(argv: Sequence[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="beatmerge", description=__doc__.splitlines()[0])
    parser.add_argument("--sources", nargs="+", required=True, type=Path,
                        help="source files (.json array / .csv / .jsonl)")
    parser.add_argument("--route", required=True, type=parse_route,
                        help="walking path as 'LAT,LNG;LAT,LNG;...'")
    parser.add_argument("--on-error", type=ErrorPolicy, choices=tuple(ErrorPolicy),
                        default=ErrorPolicy.FAIL,
                        help="malformed-row policy (default: fail — fail fast)")
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        beats = load_sources(args.sources, args.on_error)
    except (BrokenInput, MalformedRow) as exc:
        print(f"beatmerge: error: {exc}", file=sys.stderr)
        return 1
    ordered = order_by_route(dedupe(beats), args.route)
    json.dump([b.record() for b in ordered], sys.stdout)
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
