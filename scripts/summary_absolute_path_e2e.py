#!/usr/bin/env python3
"""E2E проверки итогов: абсолютный путь, newer-major и выключенная фича.

Запускает настоящий HTTP-бэкенд Podtekst (debug-сборка) с
`MR_SUMMARY_FAKE_BACKEND=1` и разреженным файлом модели, чтобы не качать
2,3 ГБ. С `--real` фейк не включается: нужны настоящие runtime и модель
(через `MR_SUMMARY_MODEL`/каталог данных приложения).

Требуется собранный фронт (`cd ui/web/frontend && yarn build`) и debug-бинарник
(`cargo build -p mr-app --manifest-path src-tauri/Cargo.toml`). Запуск:
    python3 scripts/summary_absolute_path_e2e.py
"""

import argparse
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
import wave
from pathlib import Path
from urllib.parse import quote

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "src-tauri/crates/mr-models/models-manifest.json"


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_http(port: int, timeout: float = 30.0) -> None:
    deadline = time.time() + timeout
    url = f"http://127.0.0.1:{port}/api/status"
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=1) as response:
                if response.status == 200:
                    return
        except Exception:
            time.sleep(0.2)
    raise RuntimeError(f"backend did not start on port {port}")


def get_json(port: int, path: str) -> object:
    with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=5) as response:
        return json.loads(response.read().decode("utf-8"))


def make_wav(path: Path, seconds: float = 2.0) -> None:
    with wave.open(str(path), "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(16000)
        out.writeframes(b"\0\0" * int(16000 * seconds))


def make_transcript(directory: Path, stem: str, texts: list[str]) -> None:
    utterances = [
        {
            "start": round(index * 4.0, 1),
            "end": round(index * 4.0 + 3.5, 1),
            "speaker": "Спикер 1" if index % 2 == 0 else "Спикер 2",
            "channel": None,
            "text": text,
        }
        for index, text in enumerate(texts)
    ]
    data = {
        "meta": {
            "title": stem,
            "audio": f"{stem}.wav",
            "duration_sec": round(len(texts) * 4.0, 1),
            "engine": "synthetic",
            "speaker_method": "synthetic",
            "speakers": ["Спикер 1", "Спикер 2"],
            "speakers_info": [],
        },
        "stats": {"utterances": len(utterances)},
        "utterances": utterances,
    }
    json_path = directory / f"{stem}.transcript.json"
    json_path.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
    md = "\n\n".join(f"**{item['speaker']}:** {item['text']}" for item in utterances)
    (directory / f"{stem}.md").write_text(md + "\n", encoding="utf-8")


def prepare_data(base: Path, fake: bool) -> tuple[Path, Path | None]:
    data = base / "data"
    recordings = data / "recordings"
    recordings.mkdir(parents=True)
    (data / "config.json").write_text(
        json.dumps({"save_dir": str(recordings)}), encoding="utf-8"
    )

    main_texts = [
        "Коллеги, начинаем рабочую встречу и проверяем план на неделю.",
        "Первым делом обсудим подготовку отчёта и сроки его сдачи.",
        "Предлагаю перенести запуск на следующую неделю и проверить результат.",
        "Тогда зафиксируем решение, назначим ответственного и запишем задачи.",
    ]
    newer_texts = [
        "Проверяем совместимость старого формата итогов с новой версией.",
        "Нужно сохранить прежний артефакт и собрать его заново только по команде.",
        "После проверки сравним разделы и убедимся, что данные не потерялись.",
    ]
    for stem, texts in (("meeting", main_texts), ("newer", newer_texts)):
        make_wav(recordings / f"{stem}.wav")
        make_transcript(recordings, stem, texts)
    make_wav(recordings / "short.wav")
    make_transcript(
        recordings,
        "short",
        [
            "Короткая заметка для проверки предупреждения.",
            "Текста мало, но его достаточно для сборки итогов.",
        ],
    )
    make_wav(recordings / "legal.wav")
    make_transcript(
        recordings,
        "legal",
        [
            "Разбираем проект договора и проверяем сроки исполнения обязательств.",
            "Нужно уточнить подсудность и подготовить письменные возражения.",
            "Стороны согласовали порядок обмена документами по электронной почте.",
        ],
    )
    make_wav(recordings / "english.wav")
    make_transcript(
        recordings,
        "english",
        [
            "We review the quarterly plan and the remaining work.",
            "The report must be prepared before the next meeting.",
        ],
    )

    # Новее мажор: UI обязан показать явную кнопку downgrade, не трогая файл.
    for stem in ("newer", "legal"):
        (recordings / f"{stem}.summary.json").write_text(
            json.dumps(
                {
                    "schema": "podtekst.summary/v2",
                    "schema_minor": 0,
                    "sections": [{"id": "future", "title": "Будущее", "items": []}],
                },
                ensure_ascii=False,
            ),
            encoding="utf-8",
        )
        (recordings / f"{stem}.summary.md").write_text("# Будущий формат\n", encoding="utf-8")

    # Готовый английский артефакт: проверяем task-фолбэки и метаданные из API.
    golden = json.loads(
        (ROOT / "ui/web/frontend/src/lib/fixtures/summary-artifact.golden.json").read_text(
            encoding="utf-8"
        )
    )
    golden["id"] = "11111111-2222-4333-8444-555555555555"
    golden["generated_at"] = "2026-11-08T10:00:00Z"
    golden["source"]["rev"] = "stale-english-rev"
    golden["source"]["language"] = "en"
    golden["output_language"] = "en"
    golden["preset"]["label"] = "Meeting"
    golden["summary"]["sections"] = [
        {
            "id": "tasks",
            "title": "Tasks",
            "role": "task",
            "items": [
                {
                    "text": "Prepare the report.",
                    "role": "task",
                    "task": {
                        "assignee": None,
                        "deadline": None,
                        "deadline_ref": None,
                        "deadline_source": "model",
                    },
                    "refs": [],
                }
            ],
        }
    ]
    golden["summary"]["items_total"] = 1
    golden["summary"]["warnings"] = []
    (recordings / "english.summary.json").write_text(
        json.dumps(golden, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    (recordings / "english.summary.md").write_text("# Tasks\n\n- Prepare the report.\n", encoding="utf-8")

    model_path = None
    if fake:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
        model = next(item for item in manifest["models"] if item.get("kind") == "summary")
        file_info = model["files"][0]
        model_path = data / "models" / model["id"] / file_info["filename"]
        model_path.parent.mkdir(parents=True)
        with open(model_path, "wb") as handle:
            handle.truncate(file_info["size_bytes"])
        (data / "summary-config.json").write_text(
            json.dumps(
                {
                    "auto_suggest": True,
                    "default_preset": None,
                    "detail": "normal",
                    "classifier": "heuristic",
                    "enabled": True,
                }
            ),
            encoding="utf-8",
        )
    else:
        # Реальному режиму нужна установленная модель: --real + MR_SUMMARY_MODEL.
        source = os.environ.get("MR_SUMMARY_MODEL") or os.environ.get("PODTEKST_SUMMARY_MODEL")
        if source and Path(source).is_file():
            manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
            model = next(item for item in manifest["models"] if item.get("kind") == "summary")
            file_info = model["files"][0]
            target = data / "models" / model["id"] / file_info["filename"]
            target.parent.mkdir(parents=True)
            target.symlink_to(Path(source))
    return data, model_path


def set_summary_default(data: Path, preset: str, classifier: str) -> None:
    path = data / "summary-config.json"
    config = json.loads(path.read_text(encoding="utf-8")) if path.is_file() else {}
    config.update(
        {
            "auto_suggest": True,
            "default_preset": preset,
            "detail": "normal",
            "classifier": classifier,
            "enabled": True,
        }
    )
    path.write_text(json.dumps(config), encoding="utf-8")


def start_app(binary: Path, data: Path, enabled: bool, fake: bool, model: Path | None):
    port = free_port()
    env = os.environ.copy()
    env["MEETING_RECORDER_DATA_DIR"] = str(data)
    env["PODTEKST_SUMMARY_ENABLED"] = "1" if enabled else "0"
    if fake:
        env["MR_SUMMARY_FAKE_BACKEND"] = "slow"
        if model:
            env["MR_SUMMARY_MODEL"] = str(model)
    process = subprocess.Popen(
        [str(binary), "--web", "--no-browser", "--port", str(port)],
        cwd=str(ROOT),
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    try:
        wait_http(port)
    except Exception:
        process.terminate()
        output = process.stdout.read() if process.stdout else ""
        raise RuntimeError(f"app failed to start:\n{output[-4000:]}")
    return process, port


def stop_app(process: subprocess.Popen) -> None:
    process.terminate()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def recording_path(port: int, name: str) -> str:
    data = get_json(port, "/api/recordings")
    items = data.get("items") or data.get("recordings") or []
    for item in items:
        if item.get("name") == name:
            path = item.get("path")
            assert path and path.startswith("/"), f"expected absolute path, got {path!r}"
            return path
    raise AssertionError(f"{name} not found in /api/recordings")


def new_page(browser, base: str):
    context = browser.new_context(viewport={"width": 1280, "height": 900})
    context.add_init_script(
        "localStorage.setItem('mr_intro_seen','1');"
        "localStorage.setItem('mr_speakers_open','false')"
    )
    page = context.new_page()
    errors = []
    page.on("console", lambda msg: errors.append(msg.text) if msg.type == "error" else None)
    return context, page, errors


def test_absolute_path(browser, base: str, port: int, slow: bool) -> None:
    absolute = recording_path(port, "meeting.wav")
    bodies = {}
    context, page, errors = new_page(browser, base)
    page.on(
        "request",
        lambda req: bodies.__setitem__("generate", json.loads(req.post_data or "{}"))
        if "/api/summary/generate" in req.url
        else None,
    )
    page.goto(f"{base}/#/transcript?path={quote(absolute)}&video=0")
    page.wait_for_selector(".summary-block", timeout=20000)
    page.locator(".summary-actions .btn-primary").first.click()
    page.wait_for_selector(".summary-progress", timeout=10000)
    assert page.locator(".summary-progress [role=progressbar]").count() == 1
    if slow:
        # Тикер сверяется с /api/summary каждые 3 с; при рассинхроне путей
        # прогресс сменился бы на «предложение» ещё до завершения.
        page.wait_for_timeout(3500)
        assert page.locator(".summary-progress").count() == 1, "active job dropped early"
    page.wait_for_selector(".summary-result", timeout=90000)
    text = page.locator(".summary-block").inner_text()
    assert "Готово" in text and "Сгенерировано" in text
    assert page.locator(".summary-result h3").count() >= 1
    body = bodies.get("generate")
    assert body and body.get("file") == absolute, body
    assert body.get("allow_downgrade") is not True
    context.close()
    assert not errors, errors[:3]


def test_short_source(browser, base: str, port: int) -> None:
    absolute = recording_path(port, "short.wav")
    context, page, errors = new_page(browser, base)
    page.goto(f"{base}/#/transcript?path={quote(absolute)}&video=0")
    page.wait_for_selector(".summary-block", timeout=20000)
    assert "мало текста" in page.locator(".summary-block").inner_text()
    context.close()
    assert not errors, errors[:3]


def test_newer_major(browser, base: str, port: int, recordings: Path) -> None:
    absolute = recording_path(port, "newer.wav")
    artifact = recordings / "newer.summary.json"
    before = artifact.read_text(encoding="utf-8")
    bodies = {}
    context, page, errors = new_page(browser, base)
    page.on(
        "request",
        lambda req: bodies.__setitem__("generate", json.loads(req.post_data or "{}"))
        if "/api/summary/generate" in req.url
        else None,
    )
    page.goto(f"{base}/#/transcript?path={quote(absolute)}&video=0")
    page.wait_for_selector(".summary-block", timeout=20000)
    page.wait_for_selector("text=более новом формате", timeout=10000)
    text = page.locator(".summary-block").inner_text()
    assert "более новом формате" in text
    assert artifact.read_text(encoding="utf-8") == before, "artifact changed before downgrade"
    page.get_by_role("button", name="Собрать в старом формате").click()
    page.wait_for_selector(".summary-result", timeout=90000)
    body = bodies.get("generate")
    assert body and body.get("file") == absolute, body
    assert body.get("force") is True and body.get("allow_downgrade") is True, body
    assert json.loads(artifact.read_text(encoding="utf-8")).get("schema") == "podtekst.summary/v1"
    assert list(recordings.glob("newer.summary.json.newer.*")), "no .newer backup"
    context.close()
    assert not errors, errors[:3]


def test_english_task_fallback(browser, base: str, port: int) -> None:
    absolute = recording_path(port, "english.wav")
    context, page, errors = new_page(browser, base)
    page.goto(f"{base}/#/transcript?path={quote(absolute)}&video=0")
    page.wait_for_selector(".summary-block", timeout=20000)
    page.get_by_role("button", name="Открыть").click()
    page.wait_for_selector(".summary-result", timeout=10000)
    task = page.locator(".summary-task").first.inner_text()
    assert "unassigned" in task and "deadline not stated" in task, task
    assert "не назначен" not in task and "срок не назван" not in task, task
    assert page.locator(".summary-section h3").first.inner_text() == "Tasks"
    assert "Meeting" in page.locator(".summary-meta").inner_text()
    context.close()
    assert not errors, errors[:3]


def test_sensitive_downgrade(browser, base: str, port: int, recordings: Path) -> None:
    absolute = recording_path(port, "legal.wav")
    artifact = recordings / "legal.summary.json"
    before = artifact.read_text(encoding="utf-8")
    bodies = {}
    context, page, errors = new_page(browser, base)
    page.on(
        "request",
        lambda req: bodies.__setitem__("generate", json.loads(req.post_data or "{}"))
        if "/api/summary/generate" in req.url
        else None,
    )
    page.goto(f"{base}/#/transcript?path={quote(absolute)}&video=0")
    page.wait_for_selector(".summary-block", timeout=20000)
    assert "более новом формате" in page.locator(".summary-block").inner_text()
    assert artifact.read_text(encoding="utf-8") == before, "artifact changed before downgrade"
    page.get_by_role("button", name="Собрать в старом формате").click()
    page.wait_for_selector(".summary-confirm", timeout=10000)
    assert "чувствительным" in page.locator(".summary-confirm-title").inner_text()
    assert bodies.get("generate") is None, "generate started before sensitive confirmation"
    page.locator(".summary-confirm .btn-primary").click()
    page.wait_for_selector(".summary-result", timeout=90000)
    body = bodies.get("generate")
    assert body and body.get("file") == absolute, body
    assert body.get("preset_id") == "legal", body
    assert body.get("confirm_sensitive") is True, body
    assert body.get("force") is True and body.get("allow_downgrade") is True, body
    assert json.loads(artifact.read_text(encoding="utf-8")).get("schema") == "podtekst.summary/v1"
    assert list(recordings.glob("legal.summary.json.newer.*")), "no .newer backup"
    context.close()
    assert not errors, errors[:3]


def test_disabled(browser, base: str, port: int) -> None:
    absolute = recording_path(port, "meeting.wav")
    summary_requests = []
    context, page, errors = new_page(browser, base)
    page.on(
        "request",
        lambda req: summary_requests.append(req.url)
        if "/api/summary?" in req.url
        else None,
    )
    page.goto(f"{base}/#/transcript?path={quote(absolute)}&video=0")
    page.wait_for_selector(".transcript-doc", timeout=20000)
    page.wait_for_timeout(1500)
    assert page.locator(".summary-block").count() == 0, "summary panel is visible when disabled"
    assert not summary_requests, f"panel requested summary: {summary_requests}"
    page.goto(f"{base}/#/settings")
    page.wait_for_selector("text=Модели ИИ", timeout=10000)
    assert page.get_by_role("tab", name="Итоги").count() == 0, "summary tab is visible"
    context.close()
    # Проверка фичи — намеренный 503 от /api/summary/settings; браузер пишет
    # его в console, но видимого тоста быть не должно.
    errors = [error for error in errors if "503 (Service Unavailable)" not in error]
    assert not errors, errors[:3]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, help="Path to Podtekst binary")
    parser.add_argument("--real", action="store_true", help="Use the real summary model/runtime")
    parser.add_argument("--keep", action="store_true", help="Keep the temporary data directory")
    args = parser.parse_args()

    binary = args.binary or (
        ROOT / ("src-tauri/target/release/podtekst" if args.real else "src-tauri/target/debug/podtekst")
    )
    if not binary.is_file():
        print(f"missing binary: {binary}", file=sys.stderr)
        return 2
    static = ROOT / "ui/web/static/js/app.js"
    if not static.is_file():
        print("missing built frontend: run `yarn build` in ui/web/frontend", file=sys.stderr)
        return 2

    base_dir = Path(tempfile.mkdtemp(prefix="podtekst-summary-e2e-", dir="/tmp/opencode"))
    fake = not args.real
    data, model = prepare_data(base_dir, fake)
    recordings = data / "recordings"
    processes = []
    try:
        process, port = start_app(binary, data, True, fake, model)
        processes.append(process)
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(headless=True)
            base = f"http://127.0.0.1:{port}"
            try:
                test_absolute_path(browser, base, port, slow=fake)
                test_short_source(browser, base, port)
                test_newer_major(browser, base, port, recordings)
                test_english_task_fallback(browser, base, port)
            finally:
                browser.close()
        stop_app(process)
        processes.remove(process)

        set_summary_default(data, "legal", "manual_only")
        process, port = start_app(binary, data, True, fake, model)
        processes.append(process)
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(headless=True)
            base = f"http://127.0.0.1:{port}"
            try:
                test_sensitive_downgrade(browser, base, port, recordings)
            finally:
                browser.close()
        stop_app(process)
        processes.remove(process)

        process, port = start_app(binary, data, False, fake, model)
        processes.append(process)
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(headless=True)
            base = f"http://127.0.0.1:{port}"
            try:
                test_disabled(browser, base, port)
            finally:
                browser.close()
        print("summary e2e: ok")
        return 0
    finally:
        for process in processes:
            stop_app(process)
        if not args.keep:
            shutil.rmtree(base_dir, ignore_errors=True)
        else:
            print(f"kept: {base_dir}")


if __name__ == "__main__":
    raise SystemExit(main())
