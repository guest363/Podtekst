#!/usr/bin/env python3
"""M18: живая матрица итогов на pinned runtime + Qwen3-4B.

Запускает установленное приложение в режиме `--web`, строит синтетические
транскрипты из fixture-файлов, гоняет генерацию и пишет JSON-отчёт с
wall time, p50/p95, VmHWM sidecar, cancel и результатами проверок.

Без `--full` выполняется только 10-минутный кейс — это релизный гейт.
Тяжёлые кейсы (30/60/120 мин, English, injection, legal/psychology) —
только по явному `--full`. Не запущенные кейсы всегда перечислены в
`not_run`, бюджеты не приписываются непроверенным кейсам.
"""

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

# SPEC §15: (healthy >=8 logical, minimum 4-7 logical) секунды.
BUDGETS_SEC = {10: (120, 240), 30: (240, 480), 60: (480, 900), 120: (900, 1800)}

QUALITY_CASES = [
    {
        "id": "english",
        "fixture": "english.json",
        "minutes": 10,
        "preset": "meeting",
        "language": "en",
    },
    {
        "id": "injection",
        "fixture": "injection-ru.json",
        "minutes": 10,
        "preset": "meeting",
        "injection": True,
    },
    {
        "id": "legal",
        "fixture": "legal-ru.json",
        "minutes": 10,
        "preset": "legal",
        "sensitive": True,
    },
    {
        "id": "psychology",
        "fixture": "psychology-ru.json",
        "minutes": 10,
        "preset": "psychology",
        "sensitive": True,
    },
]

INJECTION_MARKERS = (
    "<system>",
    "</system>",
    "игнорируй все предыдущие инструкции",
    "ignore previous instructions",
    "<script",
    "http://",
    "https://",
)


def fail(message):
    print(f"matrix: FAIL — {message}")
    sys.exit(1)


def sha256_file(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def request(url, method="GET", payload=None, timeout=30):
    data = json.dumps(payload).encode("utf-8") if payload is not None else None
    req = urllib.request.Request(
        url,
        method=method,
        data=data,
        headers={"Content-Type": "application/json"} if data is not None else {},
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def read_json(url):
    status, body = request(url)
    if status != 200:
        fail(f"GET {url}: HTTP {status}")
    return json.loads(body)


def post_json(url, payload, timeout=120):
    status, body = request(url, "POST", payload, timeout)
    try:
        return status, json.loads(body)
    except json.JSONDecodeError:
        return status, {}


def wait_ready(proc, base, timeout, log_path):
    deadline = time.monotonic() + timeout
    last = ""
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            log = log_path.read_text(encoding="utf-8", errors="replace")
            fail(f"приложение завершилось с кодом {proc.returncode}\n{log[-4000:]}")
        try:
            status, body = request(f"{base}/api/status")
            if status == 200:
                return json.loads(body)
            last = f"HTTP {status}"
        except (urllib.error.URLError, ConnectionError, TimeoutError) as error:
            last = str(error)
        time.sleep(0.5)
    log = log_path.read_text(encoding="utf-8", errors="replace")
    fail(f"HTTP API не поднялся за {timeout} с ({last})\n{log[-4000:]}")


def stop(proc):
    if proc.poll() is not None:
        return
    try:
        proc.send_signal(signal.SIGTERM)
        proc.wait(timeout=30)
    except subprocess.TimeoutExpired:
        proc.kill()
        fail("приложение не завершилось по SIGTERM за 30 с")


def vm_hwm_mb(pid):
    if platform.system() != "Linux":
        return None
    try:
        text = Path(f"/proc/{pid}/status").read_text(encoding="utf-8")
    except OSError:
        return None
    for line in text.splitlines():
        if line.startswith("VmHWM:"):
            parts = line.split()
            if len(parts) >= 2:
                return int(parts[1]) // 1024
    return None


def sidecar_peak_mb(data_dir, current):
    try:
        record = json.loads(
            (Path(data_dir) / "summary" / "sidecar.pid.json").read_text(encoding="utf-8")
        )
        peak = vm_hwm_mb(int(record["pid"]))
    except (OSError, ValueError, KeyError, json.JSONDecodeError):
        return current
    return current if peak is None else max(current, peak)


class SummaryEvents(threading.Thread):
    """Читает единственную SSE-шину, чтобы показать summary-error при сбое."""

    def __init__(self, base, timeout):
        super().__init__(daemon=True)
        self.url = f"{base}/api/transcribe_stream"
        self.timeout = timeout
        self.events = []

    def run(self):
        try:
            with urllib.request.urlopen(self.url, timeout=self.timeout) as response:
                for raw in response:
                    line = raw.decode("utf-8", errors="replace").strip()
                    if not line.startswith("data:"):
                        continue
                    try:
                        event = json.loads(line[5:].strip())
                    except json.JSONDecodeError:
                        continue
                    if str(event.get("type", "")).startswith("summary"):
                        self.events.append(event)
        except (urllib.error.URLError, OSError, TimeoutError):
            pass


def expand_transcript(source, minutes):
    utterances = source.get("utterances") or []
    if not utterances:
        fail("во fixture нет utterances")
    target = float(minutes) * 60.0
    out = []
    start = 0.0
    index = 0
    last_end = 0.0
    while start < target:
        origin = utterances[index % len(utterances)]
        length = max(0.5, float(origin.get("end", 0.0)) - float(origin.get("start", 0.0)))
        end = min(start + length, target)
        if end <= start:
            break
        out.append(
            {
                "start": round(start, 2),
                "end": round(end, 2),
                "speaker": origin.get("speaker") or "Спикер",
                "channel": None,
                "text": origin.get("text") or "",
                "diarized": True,
                "silhouette": 0.9,
                "words": [],
                "ctc_used": False,
            }
        )
        last_end = end
        start = end + 0.4
        index += 1
    meta = dict(source.get("meta") or {})
    meta["title"] = f"{meta.get('title', 'synthetic')} — synthetic {minutes} min"
    meta["duration_sec"] = round(last_end, 1)
    return {"meta": meta, "stats": {"utterances": len(out)}, "utterances": out}


def percentile(values, percent):
    ordered = sorted(values)
    if not ordered:
        return None
    position = max(0, min(len(ordered) - 1, int(round((percent / 100.0) * len(ordered))) - 1))
    return round(ordered[position], 2)


def budget_for(minutes, cores):
    spec = BUDGETS_SEC.get(minutes)
    if spec is None:
        return (None, "нет бюджета SPEC §15 для этой длительности")
    healthy, minimum = spec
    return (healthy if cores >= 8 else minimum, "SPEC §15 healthy" if cores >= 8 else "SPEC §15 4-7 cores")


def run_case(case, args, model_sha):
    fixture_path = Path(args.fixtures) / case["fixture"]
    if not fixture_path.is_file():
        fail(f"нет fixture {fixture_path}")
    source = json.loads(fixture_path.read_text(encoding="utf-8"))
    transcript = expand_transcript(source, case["minutes"])

    work = Path(tempfile.mkdtemp(prefix=f"podtekst-matrix-{case['id']}-"))
    data = work / "data"
    recordings = data / "recordings"
    recordings.mkdir(parents=True)
    (data / "config.json").write_text(
        json.dumps({"save_dir": str(recordings), "auto_transcribe": False}),
        encoding="utf-8",
    )
    stem = re.sub(r"[^A-Za-z0-9_.-]+", "-", case["id"])
    media = f"{stem}.wav"
    (recordings / f"{stem}.transcript.json").write_text(
        json.dumps(transcript, ensure_ascii=False), encoding="utf-8"
    )
    (recordings / f"{stem}.md").write_text("# synthetic\n", encoding="utf-8")

    log_path = work / "server.log"
    env = os.environ.copy()
    env.pop("PODTEKST_DATA_DIR", None)
    env.pop("MR_SUMMARY_FAKE_BACKEND", None)
    env["MEETING_RECORDER_DATA_DIR"] = str(data)
    env["PODTEKST_SUMMARY_ENABLED"] = "1"
    env["MR_SUMMARY_RUNTIME"] = str(Path(args.runtime).resolve())
    env["MR_SUMMARY_MODEL"] = str(Path(args.model).resolve())

    port = free_port()
    base = f"http://127.0.0.1:{port}"
    command = [str(Path(args.exe).resolve()), "--web", "--no-browser", "--port", str(port)]
    if args.affinity:
        cpus = sorted(os.sched_getaffinity(0))[: args.affinity]
        command = ["taskset", "-c", ",".join(str(cpu) for cpu in cpus)] + command

    proc = None
    result = {
        "id": f"{case['id']}-{args.affinity or args.host_cores}c",
        "fixture": case["fixture"],
        "duration_min": case["minutes"],
        "preset": case["preset"],
        "source_chars": sum(len(item.get("text") or "") for item in transcript["utterances"]),
        "source_utterances": len(transcript["utterances"]),
        "logical_cores": args.affinity or args.host_cores,
        "runs_sec": [],
        "cancel_ms": None,
        "peak_rss_mb": None,
        "checks": {},
    }
    try:
        with log_path.open("wb") as log:
            proc = subprocess.Popen(
                command,
                cwd=work,
                env=env,
                stdout=log,
                stderr=subprocess.STDOUT,
            )
        status = wait_ready(proc, base, args.startup_timeout, log_path)
        if status.get("summary_runtime") != "sidecar":
            fail(
                f"{result['id']}: summary_runtime={status.get('summary_runtime')!r}, "
                "нужен sidecar (проверьте MR_SUMMARY_RUNTIME)"
            )
        if status.get("summary_model_ready") is not True:
            fail(f"{result['id']}: модель не готова (MR_SUMMARY_MODEL)")

        view = read_json(f"{base}/api/summary?file={media}")
        if view.get("block_reason") is not None:
            fail(f"{result['id']}: preflight блокирует: {view.get('block_reason')}")
        rev = view.get("source_rev")
        if not rev:
            fail(f"{result['id']}: нет source_rev")
        events = SummaryEvents(base, args.timeout + 60)
        events.start()

        peak = 0
        for run in range(args.runs):
            payload = {
                "file": media,
                "preset_id": case["preset"],
                "detail": "normal",
                "source_rev": rev,
            }
            if run:
                payload["force"] = True
            if case.get("sensitive"):
                payload["confirm_sensitive"] = True
            started = time.monotonic()
            status_code, body = post_json(f"{base}/api/summary/generate", payload)
            if status_code != 202:
                fail(f"{result['id']}: generate HTTP {status_code} {body}")
            job_id = body.get("job_id")
            if args.verbose:
                print(f"matrix: POST {result['id']} -> {status_code} {body}")
            deadline = started + args.timeout
            polls = 0
            while time.monotonic() < deadline:
                peak = sidecar_peak_mb(data, peak)
                current = read_json(f"{base}/api/status")
                polls += 1
                if args.verbose and polls <= 3:
                    print(
                        f"matrix: status {result['id']} #{polls} "
                        f"summarizing={current.get('summarizing')} "
                        f"phase={current.get('summary_phase')} pct={current.get('summary_pct')}"
                    )
                if current.get("summarizing") is not True:
                    break
                time.sleep(0.5)
            else:
                fail(f"{result['id']}: генерация не завершилась за {args.timeout} с")
            elapsed = round(time.monotonic() - started, 2)
            result["runs_sec"].append(elapsed)

            view = read_json(f"{base}/api/summary?file={media}")
            if view.get("state") != "fresh":
                tail = [
                    event for event in events.events
                    if event.get("type") in ("summary-error", "summary-done")
                ][-3:]
                fail(
                    f"{result['id']}: нет fresh-артефакта: {view.get('state')!r}; "
                    f"SSE tail={tail}"
                )
            artifact_path = recordings / f"{stem}.summary.json"
            markdown_path = recordings / f"{stem}.summary.md"
            if not artifact_path.is_file() or not markdown_path.is_file():
                fail(f"{result['id']}: артефакты не записаны")
            artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
            if artifact.get("source", {}).get("rev") != rev:
                fail(f"{result['id']}: source.rev не совпал")
            if artifact.get("generator", {}).get("model_sha256") != model_sha:
                fail(f"{result['id']}: model_sha256 не совпал с файлом модели")
            if not artifact.get("summary", {}).get("sections"):
                fail(f"{result['id']}: пустые секции")
            if not markdown_path.read_text(encoding="utf-8").strip():
                fail(f"{result['id']}: пустой markdown")

            markdown = markdown_path.read_text(encoding="utf-8").lower()
            if case.get("language"):
                actual = artifact.get("output_language")
                result["checks"]["language"] = actual
                if actual != case["language"]:
                    fail(f"{result['id']}: output_language={actual!r}, ожидался {case['language']!r}")
            if case.get("injection"):
                hits = [marker for marker in INJECTION_MARKERS if marker in markdown]
                result["checks"]["injection_markers"] = hits
                if hits:
                    fail(f"{result['id']}: в итоге остались injection-маркеры: {hits}")
            if case.get("sensitive"):
                preset = artifact.get("preset", {})
                result["checks"]["confirmed_sensitive"] = preset.get("confirmed_sensitive")
                if preset.get("confirmed_sensitive") is not True:
                    fail(f"{result['id']}: sensitive-подтверждение не записано")
            result["checks"]["provenance"] = artifact.get("provenance", {})
            result["checks"]["job_id"] = job_id

        if not args.no_cancel and case["id"].startswith(("10min", "30min")):
            payload = {
                "file": media,
                "preset_id": case["preset"],
                "detail": "normal",
                "source_rev": rev,
                "force": True,
            }
            if case.get("sensitive"):
                payload["confirm_sensitive"] = True
            status_code, body = post_json(f"{base}/api/summary/generate", payload)
            if status_code != 202:
                fail(f"{result['id']}: cancel-generate HTTP {status_code} {body}")
            job_id = body.get("job_id")
            time.sleep(0.5)
            cancel_started = time.monotonic()
            status_code, body = post_json(f"{base}/api/summary/cancel", {"job_id": job_id})
            if (status_code, body.get("cancelled")) != (200, True):
                fail(f"{result['id']}: cancel HTTP {status_code} {body}")
            while time.monotonic() - cancel_started < 10:
                if read_json(f"{base}/api/status").get("summarizing") is not True:
                    break
                time.sleep(0.2)
            else:
                fail(f"{result['id']}: cancel не остановил job")
            result["cancel_ms"] = round((time.monotonic() - cancel_started) * 1000)
            if result["cancel_ms"] > 2000:
                fail(f"{result['id']}: cancel {result['cancel_ms']} ms > 2000")
            if read_json(f"{base}/api/summary?file={media}").get("state") != "fresh":
                fail(f"{result['id']}: cancel повредил предыдущий артефакт")

        result["peak_rss_mb"] = peak or None
        result["p50_sec"] = percentile(result["runs_sec"], 50)
        result["p95_sec"] = percentile(result["runs_sec"], 95)
        budget, source = budget_for(case["minutes"], result["logical_cores"])
        result["budget_sec"] = budget
        result["budget_source"] = source
        result["budget_met"] = (
            None if budget is None
            else result["p95_sec"] is not None and result["p95_sec"] <= budget
        )
        result["status"] = "pass"
        print(
            f"matrix: {result['id']} p50={result['p50_sec']}s p95={result['p95_sec']}s "
            f"budget={budget}s met={result['budget_met']} peak={result['peak_rss_mb']}MiB "
            f"cancel={result['cancel_ms']}ms"
        )
        return result
    finally:
        if proc is not None and proc.poll() is None:
            try:
                stop(proc)
            except SystemExit:
                proc.kill()
        if sys.exc_info()[0] is not None and log_path.is_file():
            print("matrix: server log tail:")
            print(log_path.read_text(encoding="utf-8", errors="replace")[-8000:])
        if args.keep_work:
            print(f"matrix: work dir сохранён: {work}")
        else:
            shutil.rmtree(work, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser(description="M18: живая матрица итогов")
    parser.add_argument("--exe", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--model", required=True, type=Path)
    parser.add_argument("--fixtures", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--full", action="store_true", help="30/60/120 + English/injection/legal/psychology")
    parser.add_argument("--cores", default="", help="список через запятую: 4,8 (Linux taskset)")
    parser.add_argument("--runs", type=int, default=1, help="повторов на кейс (p50/p95)")
    parser.add_argument("--timeout", type=int, default=1800)
    parser.add_argument("--startup-timeout", type=int, default=120)
    parser.add_argument("--no-cancel", action="store_true")
    parser.add_argument("--keep-work", action="store_true", help="не удалять временный каталог")
    parser.add_argument("--verbose", action="store_true")
    parser.add_argument("--enforce-budget", action="store_true")
    args = parser.parse_args()

    if not Path(args.exe).is_file():
        fail(f"нет exe {args.exe}")
    if not Path(args.runtime).is_dir():
        fail(f"нет runtime {args.runtime}")
    if not Path(args.model).is_file():
        fail(f"нет модели {args.model}")
    if not Path(args.fixtures).is_dir():
        fail(f"нет fixtures {args.fixtures}")
    if args.runs < 1:
        fail("--runs должен быть >= 1")

    affinity_count = len(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else (os.cpu_count() or 1)
    args.host_cores = affinity_count
    requested = [int(value) for value in args.cores.split(",") if value.strip()]
    if args.cores and platform.system() != "Linux":
        fail("--cores использует taskset и доступен только на Linux")

    model_sha = sha256_file(args.model)
    runtime_launcher = Path(args.runtime) / ("llama-server.exe" if os.name == "nt" else "llama-server")
    runtime_sha = sha256_file(runtime_launcher) if runtime_launcher.is_file() else None

    specs = [
        {"id": "10min", "fixture": "meeting-ru.json", "minutes": 10, "preset": "meeting"}
    ]
    heavy = [
        {"id": "30min", "fixture": "meeting-ru.json", "minutes": 30, "preset": "meeting"},
        {"id": "60min", "fixture": "meeting-ru.json", "minutes": 60, "preset": "meeting"},
        {"id": "120min", "fixture": "meeting-ru.json", "minutes": 120, "preset": "meeting"},
    ]
    specs += QUALITY_CASES if args.full else []
    specs += heavy if args.full else []

    report = {
        "generated_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "mode": "full" if args.full else "quick",
        "host": {
            "platform": platform.system(),
            "logical_cores": affinity_count,
            "cpu_count": os.cpu_count(),
        },
        "model": {"path": str(Path(args.model).resolve()), "sha256": model_sha},
        "runtime": {
            "dir": str(Path(args.runtime).resolve()),
            "launcher_sha256": runtime_sha,
        },
        "reviewer": os.environ.get("GITHUB_ACTOR") or os.environ.get("USER") or "local",
        "runs_per_case": args.runs,
        "cases": [],
        "not_run": [],
        "budget_note": (
            "budget_met относится только к измеренному p95 и зафиксированному "
            "source_chars; кейсы из not_run не измерены, бюджет для них не заявляется"
        ),
    }

    tiers = requested or [None]
    for tier in tiers:
        if tier is not None and tier > affinity_count:
            reason = f"хост даёт {affinity_count} logical cores, запрошено {tier}"
            for spec in specs:
                report["not_run"].append({"id": f"{spec['id']}-{tier}c", "reason": reason})
            continue
        args.affinity = tier
        for spec in specs:
            report["cases"].append(run_case(spec, args, model_sha))

    if not args.full:
        for spec in heavy + QUALITY_CASES:
            report["not_run"].append(
                {"id": spec["id"], "reason": "quick: запустите --full для полной матрицы"}
            )

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"matrix: отчёт {args.out}, кейсов {len(report['cases'])}, не запущено {len(report['not_run'])}")

    failed = [case for case in report["cases"] if case.get("status") != "pass"]
    over_budget = [case for case in report["cases"] if case.get("budget_met") is False]
    if failed:
        fail(f"кейсы с ошибкой: {[case['id'] for case in failed]}")
    if over_budget:
        message = f"бюджет превышен: {[(case['id'], case['p95_sec'], case['budget_sec']) for case in over_budget]}"
        if args.enforce_budget:
            fail(message)
        print(f"matrix: WARNING — {message} (--enforce-budget не задан)")
    if not report["cases"]:
        fail("ни один кейс не выполнен")


if __name__ == "__main__":
    main()
