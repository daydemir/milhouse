"""Unit tests for beatmerge (python3 -m unittest discover -q)."""
import contextlib
import io
import json
import math
import pathlib
import tempfile
import unittest

import beatmerge as bm

ROOT = pathlib.Path(__file__).resolve().parent.parent
FX = ROOT / "fixtures"
ROUTE = ((37.80200, -122.26810), (37.80300, -122.25900), (37.80400, -122.25200))

# ~1 deg lat = 111,194.93 m at EARTH_RADIUS_M; handy offsets around the 25 m radius.
DEG_24M = 24.0 / bm.M_PER_DEG
DEG_26M = 26.0 / bm.M_PER_DEG


def beat(title="X", lat=37.8, lng=-122.26, **extra):
    return bm.Beat(title=title, lat=lat, lng=lng, extra=extra)


class TestNormalize(unittest.TestCase):
    def test_aliases_and_extras(self):
        b = bm.normalize({"name": "A", "latitude": 1.0, "longitude": 2.0, "text": "t"})
        self.assertEqual((b.title, b.lat, b.lng), ("A", 1.0, 2.0))
        self.assertEqual(b.extra, {"text": "t"})

    def test_canonical_keys_win_and_record_shape(self):
        b = bm.normalize({"title": "A", "lat": 1, "lng": 2, "note": "n"})
        self.assertEqual(b.record(), {"note": "n", "title": "A", "lat": 1.0, "lng": 2.0})
        self.assertIsInstance(b.record()["lat"], float)

    def test_csv_string_coords_parse(self):
        b = bm.normalize({"title": "A", "lat": "37.81", "lng": "-122.25"})
        self.assertEqual((b.lat, b.lng), (37.81, -122.25))

    def test_malformed_rows(self):
        bad = [
            {"lat": 1.0, "lng": 2.0},                        # no title
            {"title": "", "lat": 1.0, "lng": 2.0},           # empty title
            {"title": "A", "lng": 2.0},                      # missing lat
            {"title": "A", "lat": "", "lng": 2.0},           # empty CSV cell
            {"title": "A", "lat": "NaNstring", "lng": 2.0},  # non-numeric string
            {"title": "A", "lat": True, "lng": 2.0},         # bool is not a coord
            {"title": "A", "lat": float("nan"), "lng": 2.0}, # non-finite
            {"title": "A", "lat": float("inf"), "lng": 2.0},
            {"title": "A", "lat": 1.0, "lng": 2.0, None: ["x"]},  # CSV cells beyond header
            "not-a-dict",
        ]
        for raw in bad:
            with self.assertRaises(bm.MalformedRow, msg=repr(raw)):
                bm.normalize(raw)


class TestDedupe(unittest.TestCase):
    def test_first_seen_wins_within_radius(self):
        first = beat(lat=37.8, note="winner")
        near = beat(lat=37.8 + DEG_24M, note="loser")
        self.assertEqual(bm.dedupe([first, near]), [first])

    def test_boundary_25m_inclusive_26m_distinct(self):
        base = beat(lat=37.8)
        just_out = beat(lat=37.8 + DEG_26M)
        self.assertEqual(len(bm.dedupe([base, beat(lat=37.8 + DEG_24M)])), 1)
        self.assertEqual(bm.dedupe([base, just_out]), [base, just_out])

    def test_same_point_different_title_distinct(self):
        a, b = beat(title="A"), beat(title="B")
        self.assertEqual(bm.dedupe([a, b]), [a, b])

    def test_antimeridian_pair_collapses(self):
        west = beat(lat=0.0, lng=179.99999)
        east = beat(lat=0.0, lng=-179.99999)  # ~2 m away across the antimeridian
        self.assertEqual(bm.dedupe([west, east]), [west])

    def test_dominant_title_10k_under_2s(self):
        # 10k same-title beats on a ~110 m grid (all distinct): the spatial hash must
        # keep this well under task.md's 2 s bound (was O(n^2) ~15 s pre-fix).
        import time
        beats = [beat(lat=37.0 + (i // 100) * 1e-3, lng=-122.0 + (i % 100) * 1e-3)
                 for i in range(10_000)]
        t0 = time.perf_counter()
        survivors = bm.dedupe(beats)
        self.assertLess(time.perf_counter() - t0, 2.0)
        self.assertEqual(len(survivors), 10_000)


class TestRouteMetric(unittest.TestCase):
    def test_nearest_point_on_segment_beats_nearest_vertex(self):
        # Beat abeam the middle of a ~1km segment: far from both vertices,
        # ~30m from the segment itself.
        a, b = (37.800, -122.270), (37.809, -122.270)
        mid_off = (37.8045, -122.270 + 30.0 / (bm.M_PER_DEG * math.cos(math.radians(37.8045))))
        d = bm.route_distance_m(mid_off, (a, b))
        self.assertAlmostEqual(d, 30.0, delta=1.0)
        self.assertGreater(bm.haversine_m(mid_off, a), 400.0)  # nearest-vertex would say ~500m

    def test_on_vertex_is_zero(self):
        self.assertAlmostEqual(bm.route_distance_m(ROUTE[0], ROUTE), 0.0, delta=0.01)

    def test_single_vertex_route(self):
        d = bm.route_distance_m((37.80200 + DEG_26M, -122.26810), (ROUTE[0],))
        self.assertAlmostEqual(d, 26.0, delta=0.5)

    def test_order_tiebreak_by_title(self):
        p = (37.805, -122.26)
        beats = [beat(title="B", lat=p[0], lng=p[1]), beat(title="A", lat=p[0], lng=p[1])]
        self.assertEqual([x.title for x in bm.order_by_route(beats, ROUTE)], ["A", "B"])


class TestLoading(unittest.TestCase):
    def _write(self, td, name, text):
        path = pathlib.Path(td) / name
        path.write_text(text)
        return path

    def test_skip_policy_drops_bad_rows(self):
        with contextlib.redirect_stderr(io.StringIO()):
            beats = bm.load_sources(
                [FX / "beats.json", FX / "beats.csv", FX / "beats.jsonl"], bm.ErrorPolicy.SKIP)
        self.assertEqual(len(beats), 8)
        self.assertNotIn("Bad", [b.title for b in beats])

    def test_fail_policy_raises_on_bad_row(self):
        with self.assertRaises(bm.MalformedRow):
            bm.load_sources([FX / "beats.json"], bm.ErrorPolicy.FAIL)

    def test_broken_inputs_fatal_even_under_skip(self):
        with tempfile.TemporaryDirectory() as td:
            cases = [
                self._write(td, "notjson.json", "{oops"),
                self._write(td, "obj.json", '{"title": "A"}'),        # non-array top level
                self._write(td, "badhdr.csv", "a,b\n1,2\n"),          # missing required cols
                pathlib.Path(td) / "missing.jsonl",                   # unreadable
                self._write(td, "beats.txt", "x"),                    # unknown extension
            ]
            for path in cases:
                with self.assertRaises(bm.BrokenInput, msg=str(path)):
                    bm.load_sources([path], bm.ErrorPolicy.SKIP)

    def test_jsonl_bad_line_early_keeps_later_rows(self):
        # Regression: a raised (vs yielded) MalformedRow finalized the generator and
        # silently dropped every row after the bad line.
        with tempfile.TemporaryDirectory() as td:
            path = self._write(td, "b.jsonl",
                               'not json {{{\n'
                               '{"title":"A","lat":1.0,"lng":2.0}\n'
                               '{"title":"B","lat":3.0,"lng":4.0}\n')
            with contextlib.redirect_stderr(io.StringIO()):
                beats = bm.load_sources([path], bm.ErrorPolicy.SKIP)
            self.assertEqual([b.title for b in beats], ["A", "B"])
            with self.assertRaises(bm.MalformedRow):
                bm.load_sources([path], bm.ErrorPolicy.FAIL)

    def test_jsonl_bad_line_is_per_row(self):
        with tempfile.TemporaryDirectory() as td:
            path = self._write(td, "b.jsonl",
                               '{"title":"A","lat":1.0,"lng":2.0}\nnot json\n\n')
            with contextlib.redirect_stderr(io.StringIO()):
                beats = bm.load_sources([path], bm.ErrorPolicy.SKIP)
            self.assertEqual([b.title for b in beats], ["A"])
            with self.assertRaises(bm.MalformedRow):
                bm.load_sources([path], bm.ErrorPolicy.FAIL)


class TestMain(unittest.TestCase):
    ARGS = ["--sources", str(FX / "beats.json"), str(FX / "beats.csv"), str(FX / "beats.jsonl"),
            "--route", "37.80200,-122.26810;37.80300,-122.25900;37.80400,-122.25200"]

    def test_end_to_end_skip(self):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = bm.main([*self.ARGS, "--on-error", "skip"])
        self.assertEqual(code, 0)
        data = json.loads(out.getvalue())
        self.assertEqual(len(data), 6)
        self.assertEqual(data[0]["title"], "City Hall")  # ~9m from route start
        self.assertEqual(sorted(t for t in (r["title"] for r in data)).count("City Hall"), 2)

    def test_default_policy_is_fail(self):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = bm.main(self.ARGS)  # fixtures contain malformed rows
        self.assertEqual(code, 1)
        self.assertEqual(out.getvalue(), "")  # no partial JSON on stdout

    def test_bad_route_exits_2(self):
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as ctx:
                bm.main(["--sources", str(FX / "beats.json"), "--route", "garbage"])
        self.assertEqual(ctx.exception.code, 2)


if __name__ == "__main__":
    unittest.main()
