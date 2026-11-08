#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import json
import math
import sys
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Any, Iterable

EARTH_RADIUS_M = 6_371_000.0
DEDUP_M = 25.0
CELL_M = 25.0
M_PER_DEG_LAT = 111_320.0
TITLE_KEYS = ("title", "name")
LAT_KEYS = ("lat", "latitude")
LNG_KEYS = ("lng", "longitude", "lon")


class SourceFormat(Enum):
    JSON = ".json"
    CSV = ".csv"
    JSONL = ".jsonl"

    @classmethod
    def from_path(cls, path: Path) -> "SourceFormat":
        for fmt in cls:
            if path.suffix.lower() == fmt.value:
                return fmt
        raise FatalError(f"unknown source extension: {path}")


class ErrorPolicy(Enum):
    SKIP = "skip"
    FAIL = "fail"


class FatalError(Exception):
    pass


class RowError(Exception):
    pass


@dataclass(frozen=True)
class Beat:
    title: str
    lat: float
    lng: float
    fields: dict[str, Any]


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--sources", nargs="+", required=True)
    parser.add_argument("--route", required=True)
    # Default is fail-fast because silent skips can hide data loss.
    parser.add_argument("--on-error", choices=[p.value for p in ErrorPolicy], default=ErrorPolicy.FAIL.value)
    return parser.parse_args(argv)


def parse_route(raw: str) -> list[tuple[float, float]]:
    points: list[tuple[float, float]] = []
    for chunk in raw.split(";"):
        try:
            lat_raw, lng_raw = chunk.split(",", 1)
            lat, lng = float(lat_raw), float(lng_raw)
        except ValueError as exc:
            raise FatalError(f"bad route vertex: {chunk}") from exc
        if not math.isfinite(lat) or not math.isfinite(lng):
            raise FatalError(f"bad route vertex: {chunk}")
        points.append((lat, lng))
    if len(points) < 1:
        raise FatalError("route must contain at least one vertex")
    return points


def load_source(path: Path, policy: ErrorPolicy) -> list[Beat]:
    fmt = SourceFormat.from_path(path)
    try:
        if fmt is SourceFormat.JSON:
            with path.open() as fh:
                data = json.load(fh)
            if not isinstance(data, list):
                raise FatalError(f"{path}: JSON source must be an array")
            rows = enumerate(data, 1)
        elif fmt is SourceFormat.CSV:
            rows = read_csv_rows(path)
        else:
            rows = read_jsonl_rows(path, policy)
    except OSError as exc:
        raise FatalError(f"{path}: {exc.strerror}") from exc
    except UnicodeDecodeError as exc:
        raise FatalError(f"{path}: invalid text encoding: {exc}") from exc
    except json.JSONDecodeError as exc:
        raise FatalError(f"{path}: invalid JSON container: {exc}") from exc

    beats: list[Beat] = []
    try:
        for line_no, row in rows:
            try:
                if isinstance(row, RowError):
                    raise row
                beats.append(normalize_row(row))
            except RowError as exc:
                if policy is ErrorPolicy.FAIL:
                    raise FatalError(f"{path}:{line_no}: {exc}") from exc
    except OSError as exc:
        raise FatalError(f"{path}: {exc.strerror}") from exc
    except UnicodeDecodeError as exc:
        raise FatalError(f"{path}: invalid text encoding: {exc}") from exc
    except csv.Error as exc:
        raise FatalError(f"{path}: invalid CSV container: {exc}") from exc
    return beats


def read_csv_rows(path: Path) -> Iterable[tuple[int, dict[str, str] | RowError]]:
    with path.open(newline="") as fh:
        reader = csv.DictReader(fh)
        if not reader.fieldnames:
            raise FatalError(f"{path}: CSV source must have a header row")
        header = set(reader.fieldnames)
        if not (header & set(TITLE_KEYS) and header & set(LAT_KEYS) and header & set(LNG_KEYS)):
            raise FatalError(f"{path}: CSV header must include title, lat, and lng columns")
        for line_no, row in enumerate(reader, 2):
            if None in row or any(value is None for value in row.values()):
                yield line_no, RowError("short or long CSV row")
                continue
            yield line_no, row


def read_jsonl_rows(path: Path, policy: ErrorPolicy) -> Iterable[tuple[int, Any]]:
    with path.open() as fh:
        for line_no, line in enumerate(fh, 1):
            try:
                yield line_no, json.loads(line)
            except json.JSONDecodeError as exc:
                if policy is ErrorPolicy.FAIL:
                    raise FatalError(f"{path}:{line_no}: invalid JSONL row: {exc}") from exc


def normalize_row(row: Any) -> Beat:
    if not isinstance(row, dict):
        raise RowError("row must be an object")
    title = first_present(row, TITLE_KEYS)
    lat = first_present(row, LAT_KEYS)
    lng = first_present(row, LNG_KEYS)
    if not isinstance(title, str) or not title:
        raise RowError("missing or empty title")
    lat_f = number(lat, "lat")
    lng_f = number(lng, "lng")
    fields = dict(row)
    fields["title"] = title
    fields["lat"] = lat_f
    fields["lng"] = lng_f
    return Beat(title, lat_f, lng_f, fields)


def first_present(row: dict[str, Any], keys: tuple[str, ...]) -> Any:
    for key in keys:
        if key in row:
            return row[key]
    return None


def number(value: Any, name: str) -> float:
    if isinstance(value, bool) or value is None:
        raise RowError(f"bad {name}")
    try:
        parsed = float(value)
    except (TypeError, ValueError) as exc:
        raise RowError(f"bad {name}") from exc
    if not math.isfinite(parsed):
        raise RowError(f"bad {name}")
    return parsed


def dedupe(beats: Iterable[Beat]) -> list[Beat]:
    kept: list[Beat] = []
    grid: dict[tuple[str, tuple[int, int]], list[Beat]] = {}
    for beat in beats:
        cell = grid_cell(beat)
        duplicate = False
        for neighbor in neighbor_cells(cell):
            for candidate in grid.get((beat.title, neighbor), []):
                if haversine_m((beat.lat, beat.lng), (candidate.lat, candidate.lng)) <= DEDUP_M:
                    duplicate = True
                    break
            if duplicate:
                break
        if duplicate:
            continue
        kept.append(beat)
        grid.setdefault((beat.title, cell), []).append(beat)
    return kept


def grid_cell(beat: Beat) -> tuple[int, int]:
    lat_cell = math.floor(beat.lat * M_PER_DEG_LAT / CELL_M)
    lon_m = M_PER_DEG_LAT * max(math.cos(math.radians(beat.lat)), 0.01)
    lng_cell = math.floor(beat.lng * lon_m / CELL_M)
    return lat_cell, lng_cell


def neighbor_cells(cell: tuple[int, int]) -> Iterable[tuple[int, int]]:
    row, col = cell
    for dr in (-1, 0, 1):
        for dc in (-1, 0, 1):
            yield row + dr, col + dc


def haversine_m(a: tuple[float, float], b: tuple[float, float]) -> float:
    lat1, lng1 = map(math.radians, a)
    lat2, lng2 = map(math.radians, b)
    d_lat = lat2 - lat1
    d_lng = lng2 - lng1
    h = math.sin(d_lat / 2) ** 2 + math.cos(lat1) * math.cos(lat2) * math.sin(d_lng / 2) ** 2
    return 2 * EARTH_RADIUS_M * math.asin(math.sqrt(h))


def route_distance_m(beat: Beat, route: list[tuple[float, float]]) -> float:
    if len(route) == 1:
        return haversine_m((beat.lat, beat.lng), route[0])
    # Distance to the route is the minimum clamped point-to-segment distance
    # using a local equirectangular meter projection for each segment.
    return min(segment_distance_m((beat.lat, beat.lng), a, b) for a, b in zip(route, route[1:]))


def segment_distance_m(
    point: tuple[float, float], start: tuple[float, float], end: tuple[float, float]
) -> float:
    lat0 = math.radians((point[0] + start[0] + end[0]) / 3)

    def project(p: tuple[float, float]) -> tuple[float, float]:
        return (
            math.radians(p[1]) * EARTH_RADIUS_M * math.cos(lat0),
            math.radians(p[0]) * EARTH_RADIUS_M,
        )

    px, py = project(point)
    ax, ay = project(start)
    bx, by = project(end)
    dx, dy = bx - ax, by - ay
    if dx == 0 and dy == 0:
        return math.hypot(px - ax, py - ay)
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    cx, cy = ax + t * dx, ay + t * dy
    return math.hypot(px - cx, py - cy)


def merge(sources: list[Path], route: list[tuple[float, float]], policy: ErrorPolicy) -> list[dict[str, Any]]:
    beats: list[Beat] = []
    for source in sources:
        beats.extend(load_source(source, policy))
    return rows_from_beats(beats, route)


def rows_from_beats(beats: Iterable[Beat], route: list[tuple[float, float]]) -> list[dict[str, Any]]:
    survivors = dedupe(beats)
    survivors.sort(key=lambda beat: (route_distance_m(beat, route), beat.title, beat.lat, beat.lng))
    return [beat.fields for beat in survivors]


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        policy = ErrorPolicy(args.on_error)
        route = parse_route(args.route)
        rows = merge([Path(raw) for raw in args.sources], route, policy)
    except FatalError as exc:
        print(str(exc), file=sys.stderr)
        return 1
    json.dump(rows, sys.stdout)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
