#!/usr/bin/env python3
"""Пакетный прогон release-сборки Podtekst по manifest.json."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import os
import platform
import re
import signal
import shutil
import subprocess
import sys
import tempfile
import time
import wave
from datetime import datetime, timezone
from pathlib import Path

if __package__:
    from .evaluate import compute_der
    from .metrics import (
        CPWER_VERSION,
        attribution_accuracy,
        cer_details,
        cpwer,
        load_segments,
        segments_text,
        speaker_count,
        wer_details,
    )
    from .transcript_to_rttm import convert_transcript, write_rttm
else:
    from evaluate import compute_der
    from metrics import (
        CPWER_VERSION,
        attribution_accuracy,
        cer_details,
        cpwer,
        load_segments,
        segments_text,
        speaker_count,
        wer_details,
    )
    from transcript_to_rttm import convert_transcript, write_rttm

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_APP = REPO_ROOT / "src-tauri" / "target" / "release" / "podtekst"
TIME_BIN = Path("/usr/bin/time")


def _sha256(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(block)
    return hasher.hexdigest()


def _save_dev_run(directory: Path, before: set[Path], audio: Path, destination: Path) -> Path:
    candidates = []
    audio_hash = _sha256(audio)
    for manifest in set(directory.glob("*/manifest.json")) - before:
        try:
            doc = json.loads(manifest.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if not isinstance(doc, dict):
            continue
        source = doc.get("input", {})
        if (isinstance(source, dict) and source.get("sha256") == audio_hash
                and isinstance(source.get("file"), str)
                and Path(source["file"]).resolve() == audio.resolve()):
            candidates.append(manifest.parent)
    if len(candidates) != 1:
        raise ValueError(f"ожидался один dev-run с совпадающим SHA-256, найдено {len(candidates)}")
    target = destination / candidates[0].name
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(candidates[0], target, dirs_exist_ok=True)
    run_id = candidates[0].name
    matching_logs = []
    for log in (directory.parent / "dev").glob("dev-*.log"):
        contents = log.read_text(encoding="utf-8", errors="replace")
        if f"run={run_id}" in contents or re.search(r'"run"\s*:\s*"' + re.escape(run_id) + '"', contents):
            matching_logs.append(log)
    for index, log in enumerate(sorted(matching_logs)):
        shutil.copy2(log, target / f"events-{index}.log")
    return target.resolve()

COLUMNS = [
    "dataset",
    "subset",
    "file_id",
    "duration",
    "speakers_ref",
    "speakers_hyp",
    "WER",
    "CER",
    "DER",
    "SCA",
    "cpWER",
    "cpwer_version",
    "attribution",
    "wall_sec",
    "rtf",
    "peak_rss_kb",
    "exit_code",
    "error",
    "language",
    "engine",
    "ref_words",
    "wer_errors",
    "ref_chars",
    "cer_errors",
    "der_ref_sec",
    "der_miss_sec",
    "der_false_alarm_sec",
    "der_confusion_sec",
    "cpwer_ref_words",
    "cpwer_errors",
    "attribution_matched",
    "attribution_correct",
    "words_hyp",
    "transcript",
    "transcript_snapshot",
    "reference_snapshot",
    "dev_run",
    "dev_run_warning",
    "MAE",
]


def _resolve(base: Path, value: object) -> Path | None:
    if value is None or str(value).strip() == "":
        return None
    path = Path(str(value)).expanduser()
    return path if path.is_absolute() else base / path


def _safe_id(value: str) -> str:
    cleaned = re.sub(r"[^0-9A-Za-z._-]+", "_", str(value)).strip("._")
    cleaned = cleaned[:120] or "file"
    return cleaned or "file"


def load_manifest(path: Path) -> list[dict]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    records = payload.get("records") if isinstance(payload, dict) else payload
    if not isinstance(records, list):
        raise ValueError(f"{path}: нужен массив records")
    return [record for record in records if isinstance(record, dict)]


def _int_or_none(value: object) -> int | None:
    if value is None or value == "":
        return None
    try:
        return int(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return None


def _float_or_none(value: object) -> float | None:
    if value is None or value == "":
        return None
    try:
        parsed = float(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return None
    return parsed if math.isfinite(parsed) else None


def _parse_events(stdout: str) -> tuple[dict | None, dict | None]:
    events: list[dict] = []
    for line in stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict):
            events.append(event)
    done = None
    error = None
    for event in events:
        kind = event.get("type")
        if kind == "done":
            done = event
        elif kind == "error":
            error = event
    return done, error


def _parse_gnu_time(text: str) -> int | None:
    for line in text.splitlines():
        if "Maximum resident set size" not in line:
            continue
        digits = re.sub(r"[^0-9]", "", line.rsplit(":", 1)[-1])
        return int(digits) if digits else None
    return None


def _gnu_time_available() -> str | None:
    if not TIME_BIN.is_file():
        return None
    try:
        probe = subprocess.run(
            [str(TIME_BIN), "-v", "-o", os.devnull, "true"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return str(TIME_BIN) if probe.returncode == 0 else None


def _terminate(process: subprocess.Popen) -> None:
    if os.name == "nt":
        process.kill()
        return
    try:
        os.killpg(os.getpgid(process.pid), signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        process.kill()


def _run_process(command: list[str], timeout: float, time_bin: str | None) -> dict:
    time_file: Path | None = None
    wrapped = list(command)
    if time_bin:
        handle = tempfile.NamedTemporaryFile(prefix="podtekst-time-", suffix=".txt", delete=False)
        handle.close()
        time_file = Path(handle.name)
        wrapped = [time_bin, "-v", "-o", str(time_file), *command]

    kwargs: dict = {
        "stdout": subprocess.PIPE,
        "stderr": subprocess.PIPE,
        "text": True,
        "encoding": "utf-8",
        "errors": "replace",
    }
    if time_bin:
        # GNU time локализует подписи; фиксируем C, иначе peak RSS не разобрать.
        environment = os.environ.copy()
        environment["LC_ALL"] = "C"
        kwargs["env"] = environment
    if os.name != "nt":
        kwargs["start_new_session"] = True

    started = time.perf_counter()
    process = subprocess.Popen(wrapped, **kwargs)
    timed_out = False
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        _terminate(process)
        stdout, stderr = process.communicate()
    wall_sec = time.perf_counter() - started

    time_text = ""
    if time_file is not None:
        try:
            time_text = time_file.read_text(encoding="utf-8", errors="replace")
        except OSError:
            time_text = ""
        finally:
            time_file.unlink(missing_ok=True)

    return {
        "stdout": stdout or "",
        "stderr": stderr or "",
        "exit_code": -1 if timed_out else int(process.returncode or 0),
        "wall_sec": wall_sec,
        "timed_out": timed_out,
        "peak_rss_kb": _parse_gnu_time(time_text),
    }


def _probe_duration(path: Path) -> float:
    try:
        import soundfile as sf

        return float(sf.info(str(path)).duration)
    except Exception:
        pass
    try:
        with wave.open(str(path), "rb") as handle:
            frames = handle.getnframes()
            rate = handle.getframerate()
            return frames / rate if rate else 0.0
    except Exception:
        return 0.0


def _rttm_speakers(path: Path) -> dict[str, set[str]]:
    by_uri: dict[str, set[str]] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        fields = line.split()
        if len(fields) >= 8 and fields[0] == "SPEAKER":
            by_uri.setdefault(fields[1], set()).add(fields[7])
    return by_uri


def _tail(text: str, limit: int = 4000) -> str:
    return text[-limit:] if len(text) > limit else text


def _error_text(errors: list[str]) -> str:
    seen: list[str] = []
    for error in errors:
        cleaned = " ".join(str(error).split())
        if cleaned and cleaned not in seen:
            seen.append(cleaned)
    return "; ".join(seen)[:500]


def _cell(value: object) -> str:
    if value is None:
        return ""
    if isinstance(value, float):
        return "" if not math.isfinite(value) else f"{value:.6f}"
    return str(value)


def _write_failure(out: Path, safe: str, command: list[str], run: dict, stderr: str) -> None:
    log_dir = out / "failures"
    log_dir.mkdir(parents=True, exist_ok=True)
    body = (
        f"command: {json.dumps(command, ensure_ascii=False)}\n"
        f"exit_code: {run['exit_code']}\n"
        f"timed_out: {run['timed_out']}\n"
        f"wall_sec: {run['wall_sec']:.3f}\n\n"
        f"stderr (tail):\n{_tail(stderr)}\n\n"
        f"stdout (tail):\n{_tail(run.get('stdout', ''), 2000)}\n"
    )
    (log_dir / f"{safe}.log").write_text(body, encoding="utf-8")


def _run_record(
    record: dict,
    index: int,
    total: int,
    options: argparse.Namespace,
    data_root: Path,
    out: Path,
    app: Path,
    time_bin: str | None,
) -> dict:
    file_id = str(record.get("id") or "")
    dataset = str(record.get("dataset") or "unknown")
    subset = str(record.get("subset") or "")
    if not file_id:
        stem = Path(str(record.get("audio") or "file")).stem
        file_id = f"{dataset}/{subset}/{stem}".strip("/")
    safe = _safe_id(file_id)
    engine = options.engine or str(record.get("engine") or "auto")
    language = options.language or str(record.get("language") or "auto")

    errors: list[str] = []
    row: dict[str, object] = {
        "dataset": dataset,
        "subset": subset,
        "file_id": file_id,
        "language": language,
        "engine": engine,
        "duration": math.nan,
        "speakers_ref": _int_or_none(record.get("speakers")),
        "speakers_hyp": None,
        "WER": math.nan,
        "CER": math.nan,
        "DER": math.nan,
        "SCA": math.nan,
        "cpWER": math.nan,
        "attribution": math.nan,
        "wall_sec": math.nan,
        "rtf": math.nan,
        "peak_rss_kb": None,
        "exit_code": None,
        "error": "",
        "ref_words": math.nan,
        "wer_errors": math.nan,
        "ref_chars": math.nan,
        "cer_errors": math.nan,
        "der_ref_sec": math.nan,
        "der_miss_sec": math.nan,
        "der_false_alarm_sec": math.nan,
        "der_confusion_sec": math.nan,
        "cpwer_ref_words": math.nan,
        "cpwer_errors": math.nan,
        "attribution_matched": math.nan,
        "attribution_correct": math.nan,
        "words_hyp": None,
        "transcript": "",
        "MAE": math.nan,
    }

    audio = _resolve(data_root, record.get("audio"))
    if audio is None or not audio.is_file():
        errors.append(f"аудио не найдено: {audio}")
        row["error"] = _error_text(errors)
        print(f"[{index}/{total}] {file_id}: ошибка (аудио)", file=sys.stderr, flush=True)
        return row

    initial_duration = _float_or_none(record.get("duration"))
    if initial_duration is None or initial_duration <= 0:
        initial_duration = _probe_duration(audio)
    row["duration"] = initial_duration if initial_duration and initial_duration > 0 else math.nan

    expected_transcript = _resolve(data_root, record.get("transcript"))
    if expected_transcript is None:
        expected_transcript = audio.with_name(f"{audio.stem}.transcript.json")
    row["transcript"] = str(expected_transcript)
    previous_mtime = (
        expected_transcript.stat().st_mtime_ns if expected_transcript.is_file() else None
    )

    command = [
        str(app),
        "--transcribe",
        str(audio),
        "--engine",
        engine,
        "--language",
        language,
    ]
    if options.ctc:
        command += ["--ctc", options.ctc]
    dev_directory = getattr(options, "dev_runs_dir", None)
    before = set(Path(dev_directory).glob("*/manifest.json")) if dev_directory else set()
    try:
        run = _run_process(command, options.timeout, time_bin)
    except OSError as exc:
        errors.append(f"не удалось запустить приложение: {exc}")
        row["error"] = _error_text(errors)
        print(f"[{index}/{total}] {file_id}: ошибка (запуск)", file=sys.stderr, flush=True)
        return row
    row["wall_sec"] = run["wall_sec"]
    row["peak_rss_kb"] = run["peak_rss_kb"]
    row["exit_code"] = run["exit_code"]
    if dev_directory:
        logs = out / "logs"
        logs.mkdir(exist_ok=True)
        (logs / f"{safe}.stdout.log").write_text(run["stdout"], encoding="utf-8")
        (logs / f"{safe}.stderr.log").write_text(run["stderr"], encoding="utf-8")
        try:
            row["dev_run"] = str(_save_dev_run(Path(dev_directory), before, audio, out / "dev-runs" / safe))
        except (OSError, ValueError, TypeError) as exc:
            row["dev_run_warning"] = _error_text([str(exc)])
            print(f"[{index}/{total}] {file_id}: dev-артефакты не сохранены: {exc}", file=sys.stderr, flush=True)

    done, event_error = _parse_events(run["stdout"])
    process_error = run["timed_out"] or run["exit_code"] != 0
    if run["timed_out"]:
        errors.append(f"timeout {options.timeout:g} с")
    if event_error is not None:
        errors.append(f"событие error: {event_error.get('message', '')}")
    if run["exit_code"] != 0 and event_error is None and not run["timed_out"]:
        errors.append(f"exit code {run['exit_code']}")
    done_code = _int_or_none(done.get("code")) if done else None
    if done_code not in (None, 0):
        errors.append(f"done code {done_code}")
    if process_error or event_error is not None or done_code not in (None, 0):
        _write_failure(out, safe, command, run, run["stderr"])

    transcript = expected_transcript
    if done is not None and done.get("json"):
        transcript = Path(str(done["json"])).expanduser()
        if not transcript.is_absolute():
            transcript = Path.cwd() / transcript
        row["transcript"] = str(transcript)

    if not transcript.is_file():
        errors.append(f"транскрипт не найден: {transcript}")
        row["error"] = _error_text(errors)
        _write_failure(out, safe, command, run, run["stderr"])
        print(f"[{index}/{total}] {file_id}: ошибка (нет JSON)", file=sys.stderr, flush=True)
        return row
    if transcript == expected_transcript and previous_mtime is not None:
        if transcript.stat().st_mtime_ns == previous_mtime:
            errors.append("транскрипт не обновился (stale)")
            row["error"] = _error_text(errors)
            _write_failure(out, safe, command, run, run["stderr"])
            print(f"[{index}/{total}] {file_id}: ошибка (stale)", file=sys.stderr, flush=True)
            return row

    try:
        transcript_bytes = transcript.read_bytes()
        doc = json.loads(transcript_bytes)
        transcript_snapshot = out / "transcripts" / f"{safe}.json"
        transcript_snapshot.parent.mkdir(parents=True, exist_ok=True)
        transcript_snapshot.write_bytes(transcript_bytes)
        hypothesis = load_segments(transcript_snapshot)
        row["transcript_snapshot"] = str(transcript_snapshot.resolve())
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        errors.append(f"транскрипт не читается: {exc}")
        row["error"] = _error_text(errors)
        _write_failure(out, safe, command, run, run["stderr"])
        print(f"[{index}/{total}] {file_id}: ошибка (JSON)", file=sys.stderr, flush=True)
        return row

    meta = doc.get("meta") if isinstance(doc, dict) and isinstance(doc.get("meta"), dict) else {}
    hyp_speakers = speaker_count(hypothesis)
    if hyp_speakers == 0:
        for value in (done.get("speakers") if done else None, meta.get("speakers")):
            if isinstance(value, list) and value:
                hyp_speakers = len(value)
                break
    row["speakers_hyp"] = hyp_speakers
    if isinstance(done, dict) and _int_or_none(done.get("words")) is not None:
        row["words_hyp"] = _int_or_none(done.get("words"))

    duration = _float_or_none(done.get("duration_sec") if done else None)
    if duration is None or duration <= 0:
        duration = row["duration"] if math.isfinite(row["duration"]) else math.nan
    if duration is None or duration <= 0:
        duration = _probe_duration(audio)
    row["duration"] = duration if duration and duration > 0 else math.nan

    ref_rttm_value = record.get("ref_rttm")
    ref_rttm = _resolve(data_root, ref_rttm_value)
    if ref_rttm_value not in (None, "") and (ref_rttm is None or not ref_rttm.is_file()):
        errors.append(f"эталонный RTTM не найден: {ref_rttm}")
    elif ref_rttm is not None and ref_rttm.is_file():
        try:
            by_uri = _rttm_speakers(ref_rttm)
            if not by_uri:
                errors.append(f"эталонный RTTM пуст: {ref_rttm}")
                by_uri = {}
            uri = next(iter(by_uri)) if len(by_uri) == 1 else str(record.get("rttm_id") or file_id)
            conversion = convert_transcript(doc, uri)
            hyp_rttm = out / "rttm" / f"{safe}.rttm"
            write_rttm(hyp_rttm, conversion.lines)
            if by_uri:
                der = compute_der(
                    ref_rttm,
                    hyp_rttm,
                    collar=options.collar,
                    skip_overlap=options.skip_overlap,
                )
                row["DER"] = der["der"]
                row["der_ref_sec"] = der["ref_sec"]
                row["der_miss_sec"] = der["miss_sec"]
                row["der_false_alarm_sec"] = der["false_alarm_sec"]
                row["der_confusion_sec"] = der["confusion_sec"]
            if row["speakers_ref"] is None and by_uri:
                counts = [len(speakers) for speakers in by_uri.values()]
                row["speakers_ref"] = max(counts) if counts else None
        except (OSError, ValueError, RuntimeError) as exc:
            errors.append(f"DER: {exc}")

    ref_text_value = record.get("ref_text")
    ref_text = _resolve(data_root, ref_text_value)
    if ref_text_value not in (None, "") and (ref_text is None or not ref_text.is_file()):
        errors.append(f"эталонный текст не найден: {ref_text}")
    elif ref_text is not None and ref_text.is_file():
        try:
            reference_snapshot = out / "references" / f"{safe}{ref_text.suffix}"
            reference_snapshot.parent.mkdir(parents=True, exist_ok=True)
            reference_snapshot.write_bytes(ref_text.read_bytes())
            reference = load_segments(reference_snapshot)
            row["reference_snapshot"] = str(reference_snapshot.resolve())
        except (OSError, ValueError, json.JSONDecodeError) as exc:
            errors.append(f"эталонный текст не читается: {exc}")
            reference = []
        if reference:
            if row["speakers_ref"] is None:
                row["speakers_ref"] = speaker_count(reference)
            ref_string = segments_text(reference)
            hyp_string = segments_text(hypothesis)
            wer = wer_details(ref_string, hyp_string)
            cer = cer_details(ref_string, hyp_string)
            row["WER"] = wer["wer"]
            row["ref_words"] = wer["ref_words"]
            row["wer_errors"] = wer["errors"]
            row["CER"] = cer["cer"]
            row["ref_chars"] = cer["ref_chars"]
            row["cer_errors"] = cer["errors"]

            try:
                cp = cpwer(reference, hypothesis)
                row["cpWER"] = cp["cpwer"]
                row["cpwer_version"] = cp["version"]
                row["cpwer_ref_words"] = cp["ref_words"]
                row["cpwer_errors"] = cp["errors"]
            except ValueError as exc:
                errors.append(f"cpWER: {exc}")
            if speaker_count(reference) >= 2:
                attribution = attribution_accuracy(reference, hypothesis)
                row["attribution"] = attribution["accuracy"]
                row["attribution_matched"] = attribution["matched"]
                row["attribution_correct"] = attribution["correct"]

    speakers_ref = row["speakers_ref"]
    if isinstance(speakers_ref, int) and isinstance(hyp_speakers, int):
        row["SCA"] = 1.0 if speakers_ref == hyp_speakers else 0.0
        row["MAE"] = float(abs(speakers_ref - hyp_speakers))

    if math.isfinite(row["duration"]) and row["duration"] > 0:
        row["rtf"] = run["wall_sec"] / row["duration"]

    row["error"] = _error_text(errors)
    status = "ok" if not row["error"] else "ошибка"
    print(f"[{index}/{total}] {file_id}: {status}", file=sys.stderr, flush=True)
    return row


def _git_commit() -> str | None:
    try:
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
    except OSError:
        return None
    if result.returncode == 0:
        return result.stdout.strip()
    return None


def _write_run_manifest(
    path: Path,
    options: argparse.Namespace,
    app: Path,
    total: int,
    engine_override: str | None,
) -> None:
    payload = {
        "schema": 1,
        "created_utc": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "app": str(app),
        "app_sha256": _sha256(app),
        "cpwer_version": CPWER_VERSION,
        "diar_bench": os.environ.get("MR_DIAR_BENCH"),
        "diar_experiment": os.environ.get("MR_DIAR_EXPERIMENT"),
        "dev_runs_dir": str(options.dev_runs_dir) if getattr(options, "dev_runs_dir", None) else None,
        "engine_override": engine_override or "from manifest",
        "language": options.language or "from manifest",
        "ctc": options.ctc or "auto",
        "collar": options.collar,
        "skip_overlap": bool(options.skip_overlap),
        "timeout_sec": options.timeout,
        "git_commit": _git_commit(),
        "python": platform.python_version(),
        "platform": platform.system(),
        "records": total,
    }
    path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8"
    )


def run_batch(options: argparse.Namespace) -> dict:
    manifest = Path(options.manifest)
    if not manifest.is_file():
        raise FileNotFoundError(f"манифест не найден: {manifest}")
    records = load_manifest(manifest)
    if options.dataset:
        records = [record for record in records if record.get("dataset") in options.dataset]
    if options.limit is not None:
        if options.limit < 0:
            raise ValueError("--limit не может быть отрицательным")
        records = records[: options.limit]
    if options.timeout <= 0:
        raise ValueError("--timeout должен быть больше нуля")
    dev_runs_dir = getattr(options, "dev_runs_dir", None)
    if dev_runs_dir and not Path(dev_runs_dir).is_dir():
        raise FileNotFoundError(f"каталог dev-run не найден: {dev_runs_dir}")

    data_root = Path(options.data_root) if options.data_root else manifest.parent
    out = Path(options.out) if options.out else Path("runs") / (
        f"{datetime.now().strftime('%Y-%m-%d')}-{(_git_commit() or 'local')[:7]}"
    )
    metrics_path = out / "metrics.csv"
    if metrics_path.exists() and not getattr(options, "overwrite", False):
        raise FileExistsError(
            f"результаты уже существуют: {metrics_path}; "
            "задайте новый --out или явно --overwrite"
        )
    out.mkdir(parents=True, exist_ok=True)
    (out / "rttm").mkdir(exist_ok=True)
    (out / "failures").mkdir(exist_ok=True)

    app = (Path(options.app) if options.app else DEFAULT_APP).resolve()
    if not app.is_file():
        raise FileNotFoundError(
            f"release-бинарник не найден: {app}\n"
            "соберите: cargo build --release -p mr-app "
            "(или задайте --app)"
        )
    if os.name != "nt" and not os.access(app, os.X_OK):
        raise PermissionError(f"бинарник не исполняемый: {app}")

    run_manifest = out / "manifest.json"
    if run_manifest.resolve() == manifest.resolve():
        run_manifest = out / "run-manifest.json"
    _write_run_manifest(
        run_manifest,
        options,
        app,
        len(records),
        options.engine,
    )
    time_bin = _gnu_time_available()
    failed = 0
    with metrics_path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=COLUMNS)
        writer.writeheader()
        for index, record in enumerate(records, 1):
            row = _run_record(
                record,
                index,
                len(records),
                options,
                data_root,
                out,
                app,
                time_bin,
            )
            writer.writerow({column: _cell(row.get(column)) for column in COLUMNS})
            handle.flush()
            if row["error"]:
                failed += 1

    return {"out": out, "rows": len(records), "failed": failed, "metrics": metrics_path}


def _self_test() -> int:
    assert _parse_gnu_time("Maximum resident set size (kbytes): 12345\n") == 12345
    assert _parse_gnu_time("no counter here") is None
    assert _safe_id("ami ihm/ES 2002a") == "ami_ihm_ES_2002a"
    assert _resolve(Path("/data"), "audio/x.wav") == Path("/data/audio/x.wav")

    done, error = _parse_events(
        'log line\n{"type":"done","code":0,"json":"/tmp/x.json"}\n'
        '{"type":"error","message":"bad"}\n'
    )
    assert done is not None and done["json"] == "/tmp/x.json"
    assert error is not None and error["message"] == "bad"

    try:
        import pyannote.metrics  # noqa: F401

        has_pyannote = True
    except ImportError:
        has_pyannote = False

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp) / "data"
        (root / "audio").mkdir(parents=True)
        (root / "ref").mkdir(parents=True)
        out = Path(tmp) / "run"

        audio = root / "audio" / "one.wav"
        with wave.open(str(audio), "wb") as handle:
            handle.setnchannels(1)
            handle.setsampwidth(2)
            handle.setframerate(16000)
            handle.writeframes(b"\x00\x00" * 32000)
        (root / "ref" / "one.rttm").write_text(
            "SPEAKER one 1 0.000 1.000 <NA> <NA> A <NA> <NA>\n"
            "SPEAKER one 1 1.000 1.000 <NA> <NA> B <NA> <NA>\n",
            encoding="utf-8",
        )
        (root / "ref" / "one.transcript.json").write_text(
            json.dumps(
                {
                    "utterances": [
                        {
                            "speaker": "A",
                            "start": 0.0,
                            "end": 1.0,
                            "text": "альфа",
                            "words": [{"w": "альфа", "start_sec": 0.0, "end_sec": 0.9}],
                        },
                        {
                            "speaker": "B",
                            "start": 1.0,
                            "end": 2.0,
                            "text": "бета",
                            "words": [{"w": "бета", "start_sec": 1.0, "end_sec": 1.9}],
                        },
                    ]
                }
            ),
            encoding="utf-8",
        )

        fake_app = Path(tmp) / "fake_podtekst.py"
        fake_app.write_text(
            f"#!{sys.executable}\n"
            "import json, sys, time\n"
            "from pathlib import Path\n"
            "args = sys.argv\n"
            "audio = Path(args[args.index('--transcribe') + 1])\n"
            "time.sleep(0.02)\n"
            "transcript = audio.with_name(audio.stem + '.transcript.json')\n"
            "doc = {'meta': {'speakers': ['A', 'B'], 'duration': '00:00:02'},\n"
            "       'utterances': [\n"
            "           {'speaker': 'A', 'start': 0.0, 'end': 1.0, 'text': 'альфа',\n"
            "            'words': [{'w': 'альфа', 'start_sec': 0.0, 'end_sec': 0.9}]},\n"
            "           {'speaker': 'B', 'start': 1.0, 'end': 2.0, 'text': 'бета',\n"
            "            'words': [{'w': 'бета', 'start_sec': 1.0, 'end_sec': 1.9}]}]}\n"
            "transcript.write_text(json.dumps(doc, ensure_ascii=False), encoding='utf-8')\n"
            "print('fake app stderr', file=sys.stderr)\n"
            "print(json.dumps({'type': 'done', 'code': 0, 'json': str(transcript.resolve()),\n"
            "                  'duration_sec': 2.0, 'speakers': ['A', 'B'], 'words': 2},\n"
            "                 ensure_ascii=False))\n",
            encoding="utf-8",
        )
        fake_app.chmod(0o755)

        manifest = root / "manifest.json"
        manifest.write_text(
            json.dumps(
                {
                    "records": [
                        {
                            "id": "one",
                            "audio": "audio/one.wav",
                            "dataset": "synthetic",
                            "subset": "test",
                            "ref_rttm": "ref/one.rttm",
                            "ref_text": "ref/one.transcript.json",
                            "language": "ru",
                            "speakers": 2,
                            "duration": 2.0,
                        }
                    ]
                }
            ),
            encoding="utf-8",
        )

        options = argparse.Namespace(
            manifest=str(manifest),
            data_root=str(root),
            out=str(out),
            app=str(fake_app),
            engine=None,
            language=None,
            ctc=None,
            limit=None,
            timeout=60.0,
            collar=0.0,
            skip_overlap=False,
            dataset=None,
        )
        result = run_batch(options)
        assert result["rows"] == 1 and result["failed"] == 0, result
        with (out / "metrics.csv").open(encoding="utf-8") as handle:
            rows = list(csv.DictReader(handle))
        assert len(rows) == 1, rows
        row = rows[0]
        assert row["exit_code"] == "0", row
        assert row["file_id"] == "one"
        assert row["speakers_ref"] == "2" and row["speakers_hyp"] == "2"
        assert abs(float(row["WER"])) < 1e-9, row["WER"]
        assert abs(float(row["CER"])) < 1e-9, row["CER"]
        assert abs(float(row["cpWER"])) < 1e-9, row["cpWER"]
        assert abs(float(row["attribution"]) - 1.0) < 1e-9, row["attribution"]
        assert float(row["rtf"]) > 0
        if has_pyannote:
            assert abs(float(row["DER"])) < 1e-9, row["DER"]
        else:
            assert row["DER"] == ""
        assert (out / "manifest.json").is_file()

    print("self-test ok")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", default="manifest.json", type=Path)
    parser.add_argument("--data-root", type=Path)
    parser.add_argument("--out", type=Path, default=None)
    parser.add_argument("--overwrite", action="store_true", help="явно разрешить замену результатов")
    parser.add_argument("--dev-runs-dir", type=Path, help="скопировать dev-артефакты каждого файла в результаты")
    parser.add_argument("--app", type=Path, help="release-бинарник podtekst")
    parser.add_argument("--engine", choices=["auto", "gigaam", "whisper"])
    parser.add_argument("--language")
    parser.add_argument("--ctc", choices=["auto", "off"], help="CTC-уточнение GigaAM")
    parser.add_argument("--limit", type=int)
    parser.add_argument("--timeout", type=float, default=3600.0)
    parser.add_argument("--collar", type=float, default=0.0)
    parser.add_argument("--skip-overlap", action="store_true")
    parser.add_argument("--dataset", action="append")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)

    if args.self_test:
        return _self_test()
    try:
        result = run_batch(args)
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"ошибка: {exc}", file=sys.stderr)
        return 2
    print(
        f"{result['out']}: файлов={result['rows']} ошибок={result['failed']} "
        f"csv={result['metrics']}"
    )
    return 0 if result["failed"] == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
