import argparse
import csv
import json
import tempfile
import unittest
from pathlib import Path

from metrics import CPWER_VERSION
from rescore_cpwer import rescore
from transcript_to_rttm import convert_transcript, write_rttm


def _document(merged=False):
    return {"utterances": [
        {"speaker": "A", "start": 0, "end": 1, "text": "альфа"},
        {"speaker": "A" if merged else "B", "start": 1, "end": 2, "text": "бета"},
    ]}


class RescoreCpwerTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.run = self.root / "run"
        self.run.mkdir()
        self.reference = self.root / "reference.json"
        self.reference.write_text(json.dumps(_document()), encoding="utf-8")
        self.snapshot = self.run / "snapshot.json"
        self.snapshot.write_text(json.dumps(_document(merged=True)), encoding="utf-8")
        self.live = self.root / "live.json"
        self.live.write_text(json.dumps(_document()), encoding="utf-8")
        self.records = [{"id": "sample", "language": "ru", "ref_text": "reference.json"}]
        self.manifest = self.root / "manifest.json"
        self.row = {
            "file_id": "sample", "language": "ru", "exit_code": "0", "error": "",
            "cpWER": "0.000000", "cpwer_errors": "0", "cpwer_ref_words": "2",
            "WER": "0.250000", "DER": "0.500000", "wall_sec": "5.000000",
            "transcript": str(self.live), "transcript_snapshot": str(self.snapshot),
        }
        self.options = argparse.Namespace(
            run=self.run, manifest=self.manifest, data_root=None,
            out=self.root / "rescored.csv", language="ru", allow_live_transcripts=False,
        )

    def _save(self, rows=None):
        self.manifest.write_text(json.dumps({"records": self.records}), encoding="utf-8")
        rows = rows if rows is not None else [self.row]
        with (self.run / "metrics.csv").open("w", encoding="utf-8", newline="") as handle:
            writer = csv.DictWriter(handle, fieldnames=list(self.row))
            writer.writeheader()
            writer.writerows(rows)

    def _rescore(self):
        self._save()
        result = rescore(self.options)
        with self.options.out.open(encoding="utf-8", newline="") as handle:
            rows = list(csv.DictReader(handle))
        return result, rows

    def test_defaults_to_russian_and_preserves_other_metrics_and_old_cpwer(self):
        english = dict(self.row, file_id="other", language="en")
        self.records.append({"id": "other", "language": "en", "ref_text": "reference.json"})
        self._save([self.row, english])
        original = (self.run / "metrics.csv").read_bytes()
        result = rescore(self.options)
        with self.options.out.open(encoding="utf-8", newline="") as handle:
            rows = list(csv.DictReader(handle))
        self.assertEqual(result["rows"], 1)
        row = rows[0]
        self.assertEqual(float(row["cpWER"]), 1.0)
        self.assertEqual(row["cpwer_errors"], "2")
        self.assertEqual(row["cpwer_version"], CPWER_VERSION)
        self.assertEqual(row["previous_cpWER"], "0.000000")
        self.assertEqual(row["previous_cpwer_version"], "")
        for column in ("WER", "DER", "wall_sec", "transcript"):
            self.assertEqual(row[column], self.row[column])
        self.assertEqual(row["rescore_hypothesis_source"], "transcript_snapshot")
        self.assertEqual((self.run / "metrics.csv").read_bytes(), original)

    def test_copied_dev_run_is_used_before_the_live_transcript(self):
        self.row["transcript_snapshot"] = ""
        dev_run = self.run / "dev-runs" / "sample"
        (dev_run / "transcript").mkdir(parents=True)
        (dev_run / "transcript" / "post.json").write_bytes(self.snapshot.read_bytes())
        self.row["dev_run"] = str(dev_run)
        _, rows = self._rescore()
        self.assertEqual(rows[0]["rescore_hypothesis_source"], "dev_run")
        self.assertEqual(float(rows[0]["cpWER"]), 1.0)

    def test_reference_snapshot_is_used_before_the_manifest_reference(self):
        self.row["reference_snapshot"] = str(self.reference)
        self.records[0]["ref_text"] = "missing.json"
        _, rows = self._rescore()
        self.assertEqual(rows[0]["rescore_reference_source"], "reference_snapshot")

    def test_live_transcript_requires_explicit_flag_and_matching_geometry(self):
        self.row["transcript_snapshot"] = ""
        self._save()
        with self.assertRaisesRegex(ValueError, "allow-live-transcripts"):
            rescore(self.options)
        self.assertFalse(self.options.out.exists())
        self.options.allow_live_transcripts = True
        rttm = self.run / "rttm" / "sample.rttm"
        write_rttm(rttm, convert_transcript(_document(), "sample").lines)
        changed = _document()
        changed["utterances"][0]["text"] = "гамма"
        self.live.write_text(json.dumps(changed), encoding="utf-8")
        result = rescore(self.options)
        self.assertEqual(result["live"], 1)
        with self.options.out.open(encoding="utf-8", newline="") as handle:
            row = next(csv.DictReader(handle))
        self.assertEqual(row["rescore_hypothesis_source"], "live_geometry_only")
        self.assertEqual(row["rescore_geometry_verified"], "true")
        self.assertEqual(float(row["cpWER"]), 0.5)

    def test_live_geometry_mismatch_is_rejected(self):
        self.row["transcript_snapshot"] = ""
        self.options.allow_live_transcripts = True
        write_rttm(self.run / "rttm" / "sample.rttm", convert_transcript(_document(merged=True), "sample").lines)
        self._save()
        with self.assertRaisesRegex(ValueError, "геометрия"):
            rescore(self.options)
        self.assertFalse(self.options.out.exists())

    def test_denominator_error_and_missing_snapshot_are_rejected_before_writing(self):
        for changes, error in (
            ({"cpwer_ref_words": "3"}, "знаменатель"),
            ({"error": "failed"}, "ошибкой"),
            ({"transcript_snapshot": str(self.root / "missing.json")}, "не найден"),
        ):
            with self.subTest(changes=changes):
                row = dict(self.row, **changes)
                second = dict(row, file_id="second")
                self.records = [
                    {"id": "sample", "ref_text": "reference.json"},
                    {"id": "second", "ref_text": "reference.json"},
                ]
                self._save([self.row, second])
                with self.assertRaisesRegex(ValueError, error):
                    rescore(self.options)
                self.assertFalse(self.options.out.exists())

    def test_duplicate_ids_are_rejected(self):
        self.records.append(dict(self.records[0]))
        self._save()
        with self.assertRaisesRegex(ValueError, "повторяющийся id"):
            rescore(self.options)
        self.records.pop()
        self._save([self.row, self.row])
        with self.assertRaisesRegex(ValueError, "повторяющийся file_id"):
            rescore(self.options)
        self.assertFalse(self.options.out.exists())

    def test_existing_output_is_never_replaced(self):
        self._save()
        self.options.out.write_bytes(b"original\n")
        with self.assertRaises(FileExistsError):
            rescore(self.options)
        self.assertEqual(self.options.out.read_bytes(), b"original\n")


if __name__ == "__main__":
    unittest.main()
