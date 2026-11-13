import argparse
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from run_batch import _run_record, _save_dev_run, run_batch


class RunPreservationTest(unittest.TestCase):
    def test_scoring_uses_exact_saved_inputs_and_reports_snapshot_failures(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            audio = root / "input.wav"
            audio.write_bytes(b"audio")
            transcript = root / "input.transcript.json"
            document = {"utterances": [{"speaker": "A", "start": 0, "end": 1, "text": "альфа"}]}
            original = (json.dumps(document, ensure_ascii=False, indent=3) + "\n").encode()
            reference = root / "reference.json"
            reference.write_bytes(original)
            options = argparse.Namespace(engine=None, language=None, ctc=None, timeout=60)
            record = {"id": "sample", "audio": str(audio), "duration": 1, "ref_text": str(reference)}
            out = root / "saved"
            out.mkdir()

            def process(*args):
                transcript.write_bytes(original)
                return {
                    "stdout": json.dumps({"type": "done", "code": 0, "json": str(transcript)}),
                    "stderr": "", "exit_code": 0, "timed_out": False,
                    "wall_sec": 1.0, "peak_rss_kb": None,
                }

            with patch("run_batch._run_process", side_effect=process), patch(
                "run_batch.wer_details", return_value={"wer": 0, "ref_words": 1, "errors": 0},
            ), patch(
                "run_batch.cer_details", return_value={"cer": 0, "ref_chars": 5, "errors": 0},
            ), patch(
                "run_batch.cpwer", return_value={"cpwer": 0, "version": "speaker-stream-v2", "ref_words": 1, "errors": 0},
            ):
                row = _run_record(record, 1, 1, options, root, out, root / "app", None)
                self.assertEqual(row["error"], "")
                self.assertTrue(Path(row["transcript_snapshot"]).is_absolute())
                self.assertTrue(Path(row["reference_snapshot"]).is_absolute())
                self.assertEqual(Path(row["transcript_snapshot"]).read_bytes(), original)
                self.assertEqual(Path(row["reference_snapshot"]).read_bytes(), original)
                transcript.write_bytes(b"replaced")
                reference.write_bytes(b"replaced")
                self.assertEqual(Path(row["transcript_snapshot"]).read_bytes(), original)
                self.assertEqual(Path(row["reference_snapshot"]).read_bytes(), original)
                (out / "transcripts" / "sample.json").unlink()
                (out / "transcripts" / "sample.json").mkdir()
                failed = _run_record(record, 1, 1, options, root, out, root / "app", None)
                self.assertTrue(failed["error"])
                self.assertNotIn("transcript_snapshot", failed)

    def test_saved_artifacts_require_a_unique_new_matching_input(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            audio = root / "input.wav"
            audio.write_bytes(b"audio")
            runs = root / "runs"
            first = runs / "first"
            first.mkdir(parents=True)
            document = {"input": {"file": str(audio), "sha256": hashlib.sha256(b"audio").hexdigest()}}
            manifest = first / "manifest.json"
            manifest.write_text(json.dumps(document), encoding="utf-8")
            before = {manifest}
            with self.assertRaises(ValueError):
                _save_dev_run(runs, before, audio, root / "saved")
            second = runs / "second"
            second.mkdir()
            (second / "manifest.json").write_text(json.dumps(document), encoding="utf-8")
            saved = _save_dev_run(runs, before, audio, root / "saved")
            self.assertEqual(json.loads((saved / "manifest.json").read_text()), document)
            with self.assertRaises(ValueError):
                _save_dev_run(runs, set(), audio, root / "ambiguous")

    def test_save_dev_run_reuses_existing_target_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            audio = root / "input.wav"
            audio.write_bytes(b"audio")
            runs = root / "runs"
            run = runs / "run"
            run.mkdir(parents=True)
            document = {"input": {"file": str(audio), "sha256": hashlib.sha256(b"audio").hexdigest()}}
            (run / "manifest.json").write_text(json.dumps(document), encoding="utf-8")
            destination = root / "saved"
            first = _save_dev_run(runs, set(), audio, destination)
            (run / "extra.json").write_text("{}", encoding="utf-8")
            second = _save_dev_run(runs, set(), audio, destination)
            self.assertEqual(first, second)
            self.assertEqual((second / "manifest.json").read_text(encoding="utf-8"), json.dumps(document))
            self.assertTrue((second / "extra.json").is_file())

    def test_existing_metrics_are_rejected_before_any_output_is_replaced(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = root / "input.json"
            manifest.write_text('{"records": []}', encoding="utf-8")
            output = root / "saved"
            output.mkdir()
            metrics = output / "metrics.csv"
            original = b"saved-baseline\n"
            metrics.write_bytes(original)
            run_manifest = output / "manifest.json"
            run_manifest.write_bytes(b"saved-provenance\n")
            options = argparse.Namespace(
                manifest=manifest, out=output, dataset=None, limit=None,
                timeout=60, data_root=None,
            )
            with self.assertRaises(FileExistsError):
                run_batch(options)
            self.assertEqual(metrics.read_bytes(), original)
            self.assertEqual(run_manifest.read_bytes(), b"saved-provenance\n")


if __name__ == "__main__":
    unittest.main()
