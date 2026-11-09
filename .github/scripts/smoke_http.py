#!/usr/bin/env python3
"""Смоук установленного пакета: версия, статика, HTTP API и SIGTERM."""

import argparse
import hashlib
import json
import os
import platform
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")


def fail(message):
    print(f"Смоук не прошёл: {message}")
    sys.exit(1)


def free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def normalize_arch(value):
    value = value.lower()
    return {
        "amd64": "x86_64",
        "x64": "x86_64",
        "aarch64": "arm64",
        "arm64": "arm64",
    }.get(value, value)


def request(url, method="GET"):
    data = b"" if method == "POST" else None
    req = urllib.request.Request(url, method=method, data=data)
    try:
        with urllib.request.urlopen(req, timeout=15) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def read_json(url, method="GET"):
    status, body = request(url, method)
    if status != 200:
        fail(f"{method} {url}: HTTP {status}")
    try:
        return json.loads(body)
    except json.JSONDecodeError as e:
        fail(f"{method} {url}: не JSON ({e})")


def post_json(url, payload):
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url, method="POST", data=data, headers={"Content-Type": "application/json"}
    )
    try:
        with urllib.request.urlopen(req, timeout=15) as response:
            return response.status, json.loads(response.read())
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read())
        except json.JSONDecodeError:
            return e.code, {}


def source_rev(utterances):
    digest = hashlib.sha256()
    digest.update(b"schema=1\n")
    for utterance in utterances:
        digest.update(struct.pack("<dd", float(utterance["start"]), float(utterance["end"])))
        speaker = utterance["speaker"].encode("utf-8")
        text = utterance["text"].encode("utf-8")
        digest.update(struct.pack("<I", len(speaker)))
        digest.update(speaker)
        digest.update(struct.pack("<I", len(text)))
        digest.update(text)
    return digest.hexdigest()


def check_summary(base, data, expect_runtime):
    status = read_json(f"{base}/api/status")
    if status.get("features", {}).get("summary") is not True:
        fail("/api/status: features.summary не включён")
    if status.get("summary_supported") is not True:
        fail("/api/status: summary_supported не true")
    runtime = status.get("summary_runtime")
    if runtime not in ("sidecar", "missing"):
        fail(f"/api/status: неизвестный summary_runtime={runtime!r}")
    expected = {"bundled": "sidecar", "missing": "missing"}.get(expect_runtime)
    if expected and runtime != expected:
        fail(f"/api/status: summary_runtime={runtime!r}, ожидался {expected!r}")
    for key in ("summary_model_ready", "summary_classifier", "summarizing"):
        if key not in status:
            fail(f"/api/status: нет поля {key}")

    presets = read_json(f"{base}/api/summary/presets")
    if presets.get("ok") is not True or len(presets.get("presets", [])) != 7:
        fail("/api/summary/presets: ожидались 7 шаблонов без модели")

    missing = f"{base}/api/summary?file=smoke.wav"
    status_code, body = request(missing)
    try:
        value = json.loads(body)
    except json.JSONDecodeError:
        value = {}
    if status_code != 404 or value.get("code") != "no_transcript":
        fail(f"/api/summary без расшифровки: HTTP {status_code} {value.get('code')!r}")

    utterances = [
        {
            "start": 0.0,
            "end": 1.5,
            "speaker": "Спикер 1",
            "text": "Синтетическая реплика для проверки HTTP-контракта смоука.",
        },
        {
            "start": 1.5,
            "end": 3.0,
            "speaker": "Спикер 2",
            "text": "Вторая синтетическая реплика без реальных данных и тем.",
        },
    ]
    transcript = {"meta": {}, "stats": {}, "utterances": utterances}
    recordings = Path(status.get("recordings_dir") or data)
    recordings.mkdir(parents=True, exist_ok=True)
    (recordings / "smoke.transcript.json").write_text(
        json.dumps(transcript, ensure_ascii=False), encoding="utf-8"
    )
    rev = source_rev(utterances)
    view = read_json(f"{base}/api/summary?file=smoke.wav")
    if view.get("source_rev") != rev:
        fail("/api/summary: source_rev не совпал с контрактом digest")
    if view.get("capabilities", {}).get("runtime") != runtime:
        fail("/api/summary: capabilities.runtime не совпал со статусом")

    code, value = post_json(
        f"{base}/api/summary/generate",
        {
            "file": "smoke.wav",
            "preset_id": "universal",
            "detail": "short",
            "source_rev": rev,
        },
    )
    if code != 503:
        fail(f"/api/summary/generate без модели/runtime: HTTP {code}, ожидался 503")
    if value.get("code") not in ("model_missing", "runtime_missing", "summary_disabled"):
        fail(f"/api/summary/generate: неожиданный код {value.get('code')!r}")


def wait_ready(proc, base, timeout, log_path):
    deadline = time.monotonic() + timeout
    last = ""
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            log = log_path.read_text(encoding="utf-8", errors="replace")
            fail(f"процесс завершился с кодом {proc.returncode}\n{log[-4000:]}")
        try:
            status, body = request(f"{base}/api/status")
            if status == 200:
                return json.loads(body)
            last = f"HTTP {status}"
        except (urllib.error.URLError, ConnectionError, TimeoutError) as e:
            last = str(e)
        time.sleep(0.5)
    log = log_path.read_text(encoding="utf-8", errors="replace")
    fail(f"HTTP API не поднялся за {timeout} с ({last})\n{log[-4000:]}")


def stop(proc):
    if proc.poll() is not None:
        return
    try:
        if os.name == "posix":
            proc.send_signal(signal.SIGTERM)
        else:
            proc.terminate()
        proc.wait(timeout=20)
    except subprocess.TimeoutExpired:
        proc.kill()
        fail("процесс не завершился по SIGTERM за 20 с")


def check_static(base, static_dir):
    for local in sorted(p for p in static_dir.rglob("*") if p.is_file()):
        relative = local.relative_to(static_dir).as_posix()
        if relative == ".mr-stamp":
            continue
        status, body = request(f"{base}/{relative}")
        if status != 200:
            fail(f"статика /{relative}: HTTP {status}")
        expected = hashlib.sha256(local.read_bytes()).hexdigest()
        actual = hashlib.sha256(body).hexdigest()
        if expected != actual:
            fail(f"статика /{relative} не совпала с исходником")
    html = request(f"{base}/")[1].decode("utf-8", errors="replace")
    if 'id="app"' not in html:
        fail("index.html не содержит Svelte-шелл")


def main():
    parser = argparse.ArgumentParser(description="Смоук установленного Podtekst")
    parser.add_argument("--exe", required=True, type=Path)
    parser.add_argument("--expected-version", required=True)
    parser.add_argument("--expected-os", required=True, choices=["linux", "macos", "windows"])
    parser.add_argument("--expected-arch", required=True)
    parser.add_argument("--static-dir", required=True, type=Path)
    parser.add_argument(
        "--expect-summary-runtime",
        choices=["any", "bundled", "missing"],
        default="any",
        help="bundled — sidecar найден в бандле; missing — headless без runtime",
    )
    parser.add_argument("--timeout", type=int, default=120)
    args = parser.parse_args()

    exe = args.exe.resolve()
    static_dir = args.static_dir.resolve()
    if not exe.is_file():
        fail(f"нет файла {exe}")
    if not static_dir.is_dir():
        fail(f"нет папки статики {static_dir}")
    if normalize_arch(platform.machine()) != normalize_arch(args.expected_arch):
        fail(f"архитектура {platform.machine()}, ожидалась {args.expected_arch}")

    work = Path(tempfile.mkdtemp(prefix="podtekst-smoke-"))
    data = work / "data"
    data.mkdir()
    recordings = data / "recordings"
    recordings.mkdir()
    (data / "config.json").write_text(
        json.dumps({"save_dir": str(recordings)}), encoding="utf-8"
    )
    log_path = work / "server.log"
    env = os.environ.copy()
    env.pop("PODTEKST_DATA_DIR", None)
    env["MEETING_RECORDER_DATA_DIR"] = str(data)
    if exe.name.endswith(".AppImage"):
        env["APPIMAGE_EXTRACT_AND_RUN"] = "1"

    proc = None
    try:
        version = subprocess.run(
            [str(exe), "--version"],
            cwd=work,
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )
        output = (version.stdout + version.stderr).strip()
        if version.returncode != 0 or args.expected_version not in output:
            fail(f"--version вернул {version.returncode}: {output!r}")

        port = free_port()
        base = f"http://127.0.0.1:{port}"
        with log_path.open("wb") as log:
            proc = subprocess.Popen(
                [str(exe), "--web", "--port", str(port)],
                cwd=work,
                env=env,
                stdout=log,
                stderr=subprocess.STDOUT,
            )
        status = wait_ready(proc, base, args.timeout, log_path)
        if status.get("platform") != args.expected_os:
            fail(f"/api/status platform={status.get('platform')!r}, ожидалась {args.expected_os}")
        if status.get("core") != "rust":
            fail("/api/status не подтвердил Rust-ядро")
        if status.get("recording") is not False:
            fail("/api/status: смоук не должен начинать запись")
        if not (data / "podtekst.lock").is_file():
            fail("приложение не использовало MEETING_RECORDER_DATA_DIR")

        check_static(base, static_dir)

        models = read_json(f"{base}/api/models")
        if models.get("ok") is not True or "models" not in models:
            fail("/api/models вернул неожиданный ответ")

        check_summary(base, data, args.expect_summary_runtime)

        diagnostics = read_json(f"{base}/api/diagnostics", method="POST")
        if diagnostics.get("ok") is not True:
            fail("/api/diagnostics не собрал отчёт")
        report = diagnostics.get("path")
        if not report or not Path(report).is_file():
            fail("/api/diagnostics не вернул существующий файл")
        try:
            Path(report).resolve().relative_to(data.resolve())
        except ValueError:
            fail("/api/diagnostics записал отчёт вне папки данных")

        stop(proc)
        if proc.poll() is None:
            fail("процесс остался жив после SIGTERM")
        print(f"Смоук OK: {exe.name}, {args.expected_os}/{args.expected_arch}, "
              f"статика {len(list(static_dir.rglob('*')))} файлов")
    finally:
        if proc is not None and proc.poll() is None:
            proc.kill()
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
