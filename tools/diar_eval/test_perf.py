import argparse
import contextlib
import csv
import io
import json
import os
import stat
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import perf
import perf_ab
from run_batch import _read_perf_sidecar, _run_record


def record(rtf_norm=1.0, duration=10.0, **stages):
    values = {
        "perf_ms": 1000.0,
        "calib_ms": 50.0,
        "rtf_perf": 0.1,
        "rtf_norm": rtf_norm,
        "other_ms": 5.0,
        "peak_rss_inproc_kb": 1000,
        "input_channels": 1,
        "calib_version": 1,
        "calib_dim": 448,
        "calib_profile": "release",
        "duration": duration,
    }
    for name in perf.STAGE_NAMES:
        values[f"{name}_ms"] = stages.get(name, 100.0)
    return values


def dataset(**kwargs):
    records = {name: value for name, value in kwargs.items()}
    return {"records": records, "aggregate": perf.aggregate_records(records)}


def write_metrics(path: Path, rows: list[dict]) -> None:
    fieldnames = [
        "file_id", "requested_id", "duration", "perf_ms", "calib_ms",
        *perf.STAGE_COLUMNS,
    ]
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)


class PerfHelpersTest(unittest.TestCase):
    def test_median_aggregation_over_reps_and_files(self):
        rows = [
            {"perf_ms": "10", "calib_ms": "1"},
            {"perf_ms": "30", "calib_ms": "1"},
            {"perf_ms": "20", "calib_ms": "1"},
        ]
        summary = perf.summarize_rows(rows)
        self.assertEqual(summary["perf_ms"], 20.0)
        self.assertIsNone(summary["rtf_perf"])

        records = {"a": record(rtf_norm=1.0), "b": record(rtf_norm=3.0)}
        aggregate = perf.aggregate_records(records)
        self.assertEqual(aggregate["rtf_norm"], 2.0)

    def test_rtf_norm_is_computed_from_median_parts_not_ratio_median(self):
        rows = [
            {"perf_ms": "1000", "calib_ms": "10", "duration": "10", "rtf_norm": "100"},
            {"perf_ms": "3000", "calib_ms": "30", "duration": "10", "rtf_norm": "100"},
        ]
        summary = perf.summarize_rows(rows)
        self.assertAlmostEqual(summary["rtf_perf"], 0.2)
        self.assertAlmostEqual(summary["rtf_norm"], 10.0)

    def test_machine_id_is_stable_and_uses_bench_id(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(
            os.environ, {"MR_DIAR_BENCH_ROOT": directory}
        ):
            first = perf.machine_fingerprint()
            second = perf.machine_fingerprint()
            self.assertEqual(first["machine_id"], second["machine_id"])
            self.assertEqual(len(first["machine_id"]), 12)
            self.assertTrue(first["bench_id"])
            self.assertTrue(first["cpu"])
            path = Path(directory) / "machine-id"
            if os.name != "nt":
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)

    def test_sidecar_path_from_transcript(self):
        self.assertEqual(
            perf.perf_path_for_transcript(Path("/tmp/a.transcript.json")),
            Path("/tmp/a.perf.json"),
        )
        self.assertIsNone(perf.perf_path_for_transcript(Path("/tmp/a.json")))


class IdFilterTest(unittest.TestCase):
    def test_requested_id_for_keeps_suite_id(self):
        record = {
            "audio": "fleurs/ru_ru/legacy.wav",
            "dataset": "fleurs-ru",
            "subset": "test",
        }
        self.assertEqual(
            perf.requested_id_for(record, "fleurs/ru_ru/legacy"),
            "fleurs/ru_ru/legacy",
        )
        self.assertIsNone(perf.requested_id_for(record, "other"))

    def test_filters_canonical_id_and_legacy_audio_suffix(self):
        records = [
            {"id": "fleurs/ru_ru/canonical", "audio": "elsewhere/canonical.wav"},
            {
                "audio": "fleurs/ru_ru/legacy.wav",
                "dataset": "fleurs-ru",
                "subset": "test",
            },
        ]
        selected = perf.filter_by_ids(records, "fleurs/ru_ru/canonical")
        self.assertEqual([perf.record_id(item) for item in selected], ["fleurs/ru_ru/canonical"])
        selected = perf.filter_by_ids(records, "fleurs/ru_ru/legacy")
        self.assertEqual(len(selected), 1)
        self.assertEqual(perf.record_id(selected[0]), "fleurs-ru/test/legacy")
        with self.assertRaises(ValueError):
            perf.filter_by_ids(records, "missing")

    def test_exact_id_wins_over_suffix_of_another_record(self):
        records = [
            {"id": "fleurs/ru_ru/x", "audio": "exact/x.wav"},
            {"audio": "fleurs/ru_ru/x.wav", "dataset": "fleurs-ru", "subset": "test"},
        ]
        selected = perf.filter_by_ids(records, "fleurs/ru_ru/x")
        self.assertEqual(len(selected), 1)
        self.assertEqual(selected[0]["id"], "fleurs/ru_ru/x")
        self.assertEqual(perf.record_id(selected[0]), "fleurs/ru_ru/x")

    def test_ambiguous_audio_suffix_lists_candidates(self):
        records = [
            {"audio": "a/fleurs/ru_ru/x.wav", "dataset": "a", "subset": "test"},
            {"audio": "b/fleurs/ru_ru/x.wav", "dataset": "b", "subset": "test"},
        ]
        with self.assertRaises(ValueError) as error:
            perf.filter_by_ids(records, "fleurs/ru_ru/x")
        self.assertIn("a/test/x", str(error.exception))
        self.assertIn("b/test/x", str(error.exception))


class CheckThresholdTest(unittest.TestCase):
    def test_total_five_percent_regression_is_caught_noise_is_ignored(self):
        baseline = dataset(a=record(rtf_norm=1.0))
        noisy = dataset(a=record(rtf_norm=1.04))
        errors, _ = perf.compare_perf(baseline, noisy)
        self.assertEqual(errors, [])

        slow = dataset(a=record(rtf_norm=1.06))
        errors, _ = perf.compare_perf(baseline, slow)
        self.assertTrue(any("total rtf_norm" in error for error in errors), errors)

    def test_stage_threshold_requires_both_ratio_and_absolute_ms(self):
        baseline = dataset(a=record(asr=100.0))
        big = dataset(a=record(asr=150.0))
        errors, _ = perf.compare_perf(baseline, big)
        self.assertTrue(any("asr_ms" in error for error in errors), errors)

        baseline = dataset(a=record(asr=10.0))
        small = dataset(a=record(asr=15.0))
        errors, _ = perf.compare_perf(baseline, small)
        self.assertEqual(errors, [])

    def test_per_file_stage_noise_is_not_blocking_when_aggregate_is_flat(self):
        baseline = dataset(a=record(diar_models=100.0), b=record(diar_models=100.0))
        current = dataset(a=record(diar_models=150.0), b=record(diar_models=50.0))
        errors, _ = perf.compare_perf(baseline, current)
        self.assertEqual(errors, [])

    def test_multiple_runs_median_out_a_single_outlier(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            files = []
            for index, perf_ms in enumerate((1000, 1000, 1500)):
                path = root / f"run-{index}.csv"
                write_metrics(path, [{"file_id": "a", "duration": 10, "perf_ms": perf_ms,
                                      "calib_ms": 50, **{c: 100 for c in perf.STAGE_COLUMNS}}])
                files.append(path)
            records, runs = perf.summarize_runs(files)
            self.assertEqual(runs, 3)
            self.assertAlmostEqual(records["a"]["rtf_norm"], 2.0)
            errors, _ = perf.compare_perf(dataset(a=record(rtf_norm=2.0)),
                                          {"records": records,
                                           "aggregate": perf.aggregate_records(records)})
            self.assertEqual(errors, [])

    def test_result_key_prefers_requested_id_and_falls_back_to_file_id(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "metrics.csv"
            write_metrics(path, [
                {"file_id": "fleurs-ru/test/x", "requested_id": "fleurs/ru_ru/x",
                 "duration": 10, "perf_ms": 1000, "calib_ms": 50,
                 **{c: 100 for c in perf.STAGE_COLUMNS}},
                {"file_id": "legacy", "requested_id": "",
                 "duration": 10, "perf_ms": 1000, "calib_ms": 50,
                 **{c: 100 for c in perf.STAGE_COLUMNS}},
            ])
            rows = perf.read_metrics(path)
            self.assertIn("fleurs/ru_ru/x", rows)
            self.assertIn("legacy", rows)
            records, runs = perf.summarize_runs([path])
            self.assertEqual(runs, 1)
            self.assertIn("fleurs/ru_ru/x", records)
            self.assertIn("legacy", records)

    def test_old_baseline_by_file_id_still_compares(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "metrics.csv"
            write_metrics(path, [{"file_id": "legacy", "duration": 10,
                                  "perf_ms": 1000, "calib_ms": 50,
                                  **{c: 100 for c in perf.STAGE_COLUMNS}}])
            records, _ = perf.summarize_runs([path])
            current = {"records": records, "aggregate": perf.aggregate_records(records)}
            baseline = dataset(legacy=record(rtf_norm=records["legacy"]["rtf_norm"]))
            errors, _ = perf.compare_perf(baseline, current)
            self.assertEqual(errors, [])

    def test_collect_run_files_expands_rep_directories(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for rep in ("rep-01", "rep-02"):
                (root / rep).mkdir()
                write_metrics(root / rep / "metrics.csv", [{"file_id": rep,
                                                            "duration": 1,
                                                            "perf_ms": 1,
                                                            "calib_ms": 1}])
            files = perf.collect_run_files([root])
            self.assertEqual([path.parent.name for path in files], ["rep-01", "rep-02"])

    def test_calib_signature_rejects_mixed_dimensions(self):
        version, dim, profiles = perf._calib_signature(
            {"a": record(), "b": {**record(), "calib_dim": 96}}
        )
        self.assertEqual(version, 1)
        self.assertIsNone(dim)
        self.assertEqual(profiles, {"release"})


class CapturePolicyTest(unittest.TestCase):
    def test_merge_keeps_other_machines_and_rejects_other_suite(self):
        baseline = {"schema": 1, "machines": {"other": {"suite": "perf", "x": 1}}}
        perf.merge_baseline(baseline, "mine", {"suite": "perf"}, "perf")
        self.assertEqual(baseline["machines"]["other"], {"suite": "perf", "x": 1})
        with self.assertRaises(ValueError):
            perf.merge_baseline(baseline, "mine", {"suite": "full"}, "full")

    def test_overwrite_cleans_only_rep_directories(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            (out / "rep-01").mkdir()
            (out / "rep-02").mkdir()
            (out / "keep.txt").write_text("x", encoding="utf-8")
            perf.clean_reps(out)
            self.assertFalse((out / "rep-01").exists())
            self.assertTrue((out / "keep.txt").is_file())

    def test_non_release_capture_is_rejected(self):
        perf.require_release({"a": {"calib_profile": "release"}})
        with self.assertRaises(ValueError):
            perf.require_release({"a": {"calib_profile": "debug"}})
        with self.assertRaises(ValueError):
            perf.require_release({"a": {}})


class PerfSidecarReadTest(unittest.TestCase):
    def test_run_record_reads_perf_sidecar_and_computes_rtf_norm(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            audio = root / "fleurs/ru_ru/input.wav"
            audio.parent.mkdir(parents=True)
            audio.write_bytes(b"audio")
            transcript = audio.with_name("input.transcript.json")
            sidecar = audio.with_name("input.perf.json")
            document = {"meta": {}, "utterances": []}
            out = root / "saved"
            out.mkdir()
            options = argparse.Namespace(
                engine=None, language=None, ctc=None, timeout=60, perf=True,
                collar=0.0, skip_overlap=False, ids="fleurs/ru_ru/input",
            )
            item = {"id": "legacy/test/input", "audio": str(audio), "duration": 10.0}

            def process(*args):
                self.assertEqual(args[3], {"MR_PERF": "1"})
                transcript.write_text(json.dumps(document), encoding="utf-8")
                sidecar.write_text(
                    json.dumps(
                        {
                            "schema": 1,
                            "total_ms": 5000.0,
                            "audio_sec": 5.0,
                            "other_ms": 7.0,
                            "peak_rss_kb": 123456,
                            "input_channels": 2,
                            "stages_ms": {"decode": 10.0, "asr": 4000.0},
                            "calib": {
                                "version": 1, "ms": 50.0, "dim": 448,
                                "profile": "release", "logical_cpus": 8,
                            },
                        }
                    ),
                    encoding="utf-8",
                )
                return {
                    "stdout": json.dumps({"type": "done", "code": 0, "json": str(transcript)}),
                    "stderr": "", "exit_code": 0, "timed_out": False,
                    "wall_sec": 5.0, "peak_rss_kb": None,
                }

            with patch("run_batch._run_process", side_effect=process):
                row = _run_record(item, 1, 1, options, root, out, root / "app", None)
            self.assertEqual(row["error"], "")
            self.assertEqual(row["perf_ms"], 5000.0)
            self.assertEqual(row["calib_ms"], 50.0)
            self.assertEqual(row["asr_ms"], 4000.0)
            self.assertEqual(row["input_channels"], 2)
            self.assertEqual(row["calib_dim"], 448)
            self.assertEqual(row["calib_profile"], "release")
            self.assertEqual(row["file_id"], "legacy/test/input")
            self.assertEqual(row["requested_id"], "fleurs/ru_ru/input")
            self.assertAlmostEqual(row["rtf_perf"], 1.0)
            self.assertAlmostEqual(row["rtf_norm"], 20.0)

    def test_stale_sidecar_is_ignored(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            transcript = root / "a.transcript.json"
            sidecar = root / "a.perf.json"
            transcript.write_text("{}", encoding="utf-8")
            sidecar.write_text("{}", encoding="utf-8")
            os.utime(sidecar, ns=(1, 1))
            self.assertEqual(_read_perf_sidecar(transcript), {})
            self.assertEqual(_read_perf_sidecar(root / "missing.transcript.json"), {})


class AbTest(unittest.TestCase):
    def test_alternates_warmup_pairs_and_exits_on_regression(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            app_a = root / "old"
            app_b = root / "new"
            app_a.write_text("old", encoding="utf-8")
            app_b.write_text("new", encoding="utf-8")
            out = root / "ab"
            calls: list[tuple[str, str]] = []

            def fake(app, manifest, data_root, file_id, run_out, timeout):
                variant = "a" if app.name == "old" else "b"
                calls.append((variant, file_id))
                value = "1.0" if variant == "a" else "2.0"
                row = {
                    "rtf_norm": value,
                    "perf_ms": "1000",
                    "error": "",
                    **{column: "100" for column in perf.STAGE_COLUMNS},
                }
                meta = {"commit": f"commit-{variant}", "engine": "gigaam"}
                return row, meta

            with patch("perf_ab._run_one", side_effect=fake), contextlib.redirect_stdout(
                io.StringIO()
            ):
                code = perf_ab.main([
                    "--a", str(app_a), "--b", str(app_b), "--suite", "perf",
                    "--reps", "1", "--warmup", "1", "--root", str(root),
                    "--out", str(out),
                ])
            self.assertEqual(code, 1)
            self.assertEqual([variant for variant, _ in calls[:2]], ["a", "b"])
            self.assertEqual([variant for variant, _ in calls[6:8]], ["b", "a"])
            report = json.loads((out / "ab.json").read_text(encoding="utf-8"))
            self.assertEqual(report["total"]["median"], 2.0)
            self.assertEqual(report["total"]["n"], 3)
            self.assertTrue(all(item["reps"] == 1 for item in report["files"].values()))
            self.assertEqual(report["binaries"]["a"]["commit"], "commit-a")
            self.assertEqual(report["binaries"]["b"]["commit"], "commit-b")

            with patch("perf_ab._run_one", side_effect=fake), contextlib.redirect_stdout(
                io.StringIO()
            ):
                repeated = perf_ab.main([
                    "--a", str(app_a), "--b", str(app_b), "--suite", "perf",
                    "--reps", "1", "--warmup", "1", "--root", str(root),
                    "--out", str(out),
                ])
            self.assertEqual(repeated, 1)


if __name__ == "__main__":
    unittest.main()
