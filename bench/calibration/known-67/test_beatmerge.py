import json
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

import beatmerge


ROUTE = [(0.0, 0.0), (0.0, 1.0)]


class BeatmergeTests(unittest.TestCase):
    def test_json_loader_and_aliases(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "beats.json"
            path.write_text(json.dumps([{"name": "A", "latitude": "1.5", "longitude": "2.5", "x": 1}]))
            beats = beatmerge.load_source(path, beatmerge.ErrorPolicy.FAIL)
        self.assertEqual(beats[0].title, "A")
        self.assertEqual(beats[0].lat, 1.5)
        self.assertEqual(beats[0].lng, 2.5)
        self.assertEqual(beats[0].fields["x"], 1)

    def test_csv_loader(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "beats.csv"
            path.write_text("title,lat,lng,note\nA,1,2,ok\n")
            beats = beatmerge.load_source(path, beatmerge.ErrorPolicy.FAIL)
        self.assertEqual(beats[0].fields["note"], "ok")

    def test_jsonl_loader(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "beats.jsonl"
            path.write_text('{"title":"A","lat":1,"lon":2}\n')
            beats = beatmerge.load_source(path, beatmerge.ErrorPolicy.FAIL)
        self.assertEqual((beats[0].title, beats[0].lat, beats[0].lng), ("A", 1.0, 2.0))

    def test_dedupe_boundary(self):
        beats = [
            beatmerge.normalize_row({"title": "A", "lat": 0, "lng": 0}),
            beatmerge.normalize_row({"title": "A", "lat": 0.000224, "lng": 0}),
            beatmerge.normalize_row({"title": "A", "lat": 0.000226, "lng": 0}),
        ]
        self.assertEqual(len(beatmerge.dedupe(beats)), 2)

    def test_route_distance_uses_segment_not_vertex(self):
        mid = beatmerge.normalize_row({"title": "mid", "lat": 0.001, "lng": 0.5})
        vertex = beatmerge.normalize_row({"title": "vertex", "lat": 0.002, "lng": 0.0})
        rows = beatmerge.rows_from_beats([vertex, mid], ROUTE)
        self.assertEqual([row["title"] for row in rows], ["mid", "vertex"])

    def test_tie_break_determinism(self):
        beats = [
            beatmerge.normalize_row({"title": "B", "lat": 0, "lng": 0.5}),
            beatmerge.normalize_row({"title": "A", "lat": 0, "lng": 0.5}),
        ]
        rows = beatmerge.rows_from_beats(beats, ROUTE)
        self.assertEqual([row["title"] for row in rows], ["A", "B"])

    def test_skip_vs_fail_per_row(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "beats.jsonl"
            path.write_text('{"title":"A","lat":1,"lng":2}\nnot-json\n')
            self.assertEqual(len(beatmerge.load_source(path, beatmerge.ErrorPolicy.SKIP)), 1)
            with self.assertRaises(beatmerge.FatalError):
                beatmerge.load_source(path, beatmerge.ErrorPolicy.FAIL)

    def test_bad_csv_row_obeys_error_policy(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "beats.csv"
            path.write_text("title,lat,lng\nA,1,2,extra\nB,3,4\n")
            self.assertEqual(len(beatmerge.load_source(path, beatmerge.ErrorPolicy.SKIP)), 1)
            with self.assertRaises(beatmerge.FatalError):
                beatmerge.load_source(path, beatmerge.ErrorPolicy.FAIL)

    def test_short_csv_row_obeys_error_policy(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "beats.csv"
            path.write_text("title,lat,lng,note\nA,1,2\nB,3,4,ok\n")
            self.assertEqual(len(beatmerge.load_source(path, beatmerge.ErrorPolicy.SKIP)), 1)
            with self.assertRaises(beatmerge.FatalError):
                beatmerge.load_source(path, beatmerge.ErrorPolicy.FAIL)

    def test_data_only_csv_is_structural_fatal(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "beats.csv"
            path.write_text("A,1,2\n")
            with self.assertRaises(beatmerge.FatalError):
                beatmerge.load_source(path, beatmerge.ErrorPolicy.SKIP)

    def test_structural_fatal_has_empty_stdout(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "beats.txt"
            path.write_text("bad")
            proc = subprocess.run(
                [
                    sys.executable,
                    str(Path(__file__).with_name("beatmerge.py")),
                    "--sources",
                    str(path),
                    "--route",
                    "0,0;0,1",
                    "--on-error",
                    "skip",
                ],
                capture_output=True,
                text=True,
            )
        self.assertNotEqual(proc.returncode, 0)
        self.assertEqual(proc.stdout, "")
        self.assertIn("unknown source extension", proc.stderr)

    def test_invalid_text_encoding_is_structural_fatal(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "beats.jsonl"
            path.write_bytes(b"\xff")
            proc = subprocess.run(
                [
                    sys.executable,
                    str(Path(__file__).with_name("beatmerge.py")),
                    "--sources",
                    str(path),
                    "--route",
                    "0,0;0,1",
                    "--on-error",
                    "skip",
                ],
                capture_output=True,
                text=True,
            )
        self.assertNotEqual(proc.returncode, 0)
        self.assertEqual(proc.stdout, "")
        self.assertIn("invalid text encoding", proc.stderr)
        self.assertNotIn("Traceback", proc.stderr)

    def test_same_title_10k_far_apart_is_fast(self):
        beats = [
            beatmerge.normalize_row({"title": "A", "lat": i * 0.001, "lng": 0})
            for i in range(10_000)
        ]
        started = time.perf_counter()
        survivors = beatmerge.dedupe(beats)
        elapsed = time.perf_counter() - started
        self.assertEqual(len(survivors), 10_000)
        self.assertLess(elapsed, 2.0)


if __name__ == "__main__":
    unittest.main()
