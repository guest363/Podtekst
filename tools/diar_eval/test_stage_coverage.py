import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import stage_coverage as coverage


class StageCoverageTest(unittest.TestCase):
    def test_union_intersection_and_changes(self):
        reference = coverage.union([(0, 5), (3, 8), (10, 12)])
        first = coverage.union([(1, 4), (2, 6), (11, 13)])
        second = coverage.union([(0, 2), (5, 8), (10, 12)])
        self.assertEqual(reference, [(0, 8), (10, 12)])
        result = coverage.stage_report(reference, second, first)
        self.assertEqual(result["reference_union_sec"], 10)
        self.assertEqual(result["hypothesis_union_sec"], 7)
        self.assertEqual(result["reference_union_uncovered_sec"], 3)
        self.assertEqual(result["reference_union_covered_sec"], 7)
        self.assertEqual(result["reference_union_uncovered_intervals"], [(2, 5)])
        self.assertEqual(result["extra_union_sec"], 0)
        self.assertEqual(result["lost_reference_since_previous_sec"], 3)
        self.assertEqual(result["gained_reference_since_previous_sec"], 4)

    def test_difference_keeps_gaps_and_spanning_hypotheses(self):
        self.assertEqual(coverage.difference([(0, 2), (4, 8)], [(1, 5)]), [(0, 1), (5, 8)])
        self.assertEqual(coverage.difference([(0, 2)], []), [(0, 2)])
        self.assertEqual(coverage.difference([(0, 2)], [(0, 2)]), [])
        self.assertEqual(coverage.difference([], [(0, 2)]), [])

    def test_invalid_intervals(self):
        for interval in [(-1, 1), (1, 1), (2, 1), (0, float("nan")), (0, float("inf"))]:
            with self.subTest(interval=interval), self.assertRaises(ValueError):
                coverage.union([interval])

    def test_provenance_and_missing_stages(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            audio = root / "audio.wav"
            audio.write_bytes(b"audio")
            reference = root / "reference.rttm"
            reference.write_text("SPEAKER clip 1 0 2 <NA> <NA> s <NA> <NA>\n")
            (root / "vad").mkdir()
            (root / "diar").mkdir()
            manifest = root / "manifest.json"
            manifest.write_text(json.dumps({"input": {"sha256": "0" * 64}}))
            with self.assertRaisesRegex(ValueError, "SHA-256"):
                coverage.analyze(root, reference, audio)
            manifest.write_text(json.dumps({"input": {"sha256": hashlib.sha256(b"audio").hexdigest()}}))
            (root / "vad/ch0.segments.json").write_text(json.dumps({"raw": [[0, 2]], "grouped": [[0, 2]]}))
            with self.assertRaises(FileNotFoundError):
                coverage.analyze(root, reference, audio)
            for stage in ("asr", "acoustic", "document"):
                (root / f"diar/ch0.{stage}.json").write_text(json.dumps({"segments": [{"start": 0, "end": 1}]}))
            result = coverage.analyze(root, reference, audio)
            self.assertIsNone(result["branches"]["raw"])
            self.assertIsNone(result["stages"]["pre-cluster"])
            self.assertEqual(result["stages"]["acoustic"]["previous_stage"], "asr")
            self.assertEqual(result["stages"]["asr"]["reference_union_uncovered_sec"], 1)
            (root / "vad/ch0.frontend.segments.json").write_text(json.dumps({
                "segments": [[0, 2]], "reused_from": "raw",
            }))
            result = coverage.analyze(root, reference, audio)
            self.assertEqual(result["branches"]["frontend"]["reused_from"], "raw")
            self.assertEqual(result["branches"]["frontend"]["reference_union_uncovered_sec"], 0)

    def test_reference_requires_uri_and_channel(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "ref.rttm"
            path.write_text("SPEAKER a 1 0 1 <NA> <NA> s <NA> <NA>\nSPEAKER b 2 1 1 <NA> <NA> s <NA> <NA>\n")
            with self.assertRaises(ValueError):
                coverage.read_reference(path)
            with self.assertRaises(ValueError):
                coverage.read_reference(path, "b")
            self.assertEqual(coverage.read_reference(path, "b", "2"), ([(1.0, 2.0)], "b"))


if __name__ == "__main__":
    unittest.main()
