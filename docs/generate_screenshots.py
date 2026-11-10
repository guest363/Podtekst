#!/usr/bin/env python3
"""
Скрипт рендеринга скриншотов и гифок для документации Подтекст.
Использует headless Chrome через Chrome DevTools Protocol (CDP) и ffmpeg.
"""

import asyncio
import base64
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
import websockets

ROOT_DIR = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
DOCS_DIR = os.path.join(ROOT_DIR, "docs")
SCREENSHOTS_DIR = os.path.join(DOCS_DIR, "screenshots")
HTML_URL = f"file://{DOCS_DIR}/render_screenshots.html"

import socket

def find_free_port():
    with socket.socket() as s:
        s.bind(('', 0))
        return s.getsockname()[1]

class ChromeRunner:
    def __init__(self, port=None):
        self.port = port or find_free_port()
        self.proc = None
        self.profile_dir = tempfile.mkdtemp(prefix="chrome_shot_")

    def start(self):
        cmd = [
            "google-chrome",
            "--headless",
            "--disable-gpu",
            "--no-sandbox",
            "--disable-dev-shm-usage",
            f"--user-data-dir={self.profile_dir}",
            f"--remote-debugging-port={self.port}",
            "--window-size=1240,820",
            "--hide-scrollbars",
            "about:blank",
        ]
        self.proc = subprocess.Popen(
            cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )
        for _ in range(40):
            time.sleep(0.1)
            try:
                req = urllib.request.urlopen(f"http://127.0.0.1:{self.port}/json", timeout=1)
                tabs = json.loads(req.read().decode())
                page_tab = next(
                    (t for t in tabs if t.get("type") == "page" and "webSocketDebuggerUrl" in t),
                    None,
                )
                if page_tab:
                    return page_tab["webSocketDebuggerUrl"]
            except Exception:
                pass
        raise RuntimeError("Не удалось запустить headless Chrome и подключиться по CDP")

    def stop(self):
        if self.proc:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.proc.kill()
            self.proc = None
        if os.path.exists(self.profile_dir):
            shutil.rmtree(self.profile_dir, ignore_errors=True)


class CdpClient:
    def __init__(self, ws_url):
        self.ws_url = ws_url
        self.ws = None
        self._msg_id = 0

    async def connect(self):
        self.ws = await websockets.connect(self.ws_url, max_size=20 * 1024 * 1024)
        await self.send("Page.enable")
        await self.send("Runtime.enable")

    async def close(self):
        if self.ws:
            await self.ws.close()

    async def send(self, method, params=None):
        self._msg_id += 1
        msg_id = self._msg_id
        payload = {"id": msg_id, "method": method}
        if params:
            payload["params"] = params
        await self.ws.send(json.dumps(payload))
        while True:
            resp_str = await self.ws.recv()
            resp = json.loads(resp_str)
            if resp.get("id") == msg_id:
                return resp.get("result")

    async def navigate(self, url):
        await self.send("Page.navigate", {"url": url})
        # Подождём загрузки DOM и шрифтов
        await asyncio.sleep(0.4)

    async def evaluate(self, expression):
        res = await self.send(
            "Runtime.evaluate",
            {"expression": expression, "awaitPromise": True, "returnByValue": True},
        )
        return res.get("result", {}).get("value")

    async def set_viewport(self, width, height, device_scale=1.0):
        await self.send(
            "Emulation.setDeviceMetricsOverride",
            {
                "width": width,
                "height": height,
                "deviceScaleFactor": device_scale,
                "mobile": False,
            },
        )

    async def capture_screenshot(self, output_path):
        res = await self.send("Page.captureScreenshot", {"format": "png"})
        data = base64.b64decode(res["data"])
        with open(output_path, "wb") as f:
            f.write(data)


async def main():
    os.makedirs(SCREENSHOTS_DIR, exist_ok=True)
    runner = ChromeRunner()
    ws_url = runner.start()
    client = CdpClient(ws_url)
    await client.connect()

    try:
        print("[1/6] Загрузка шаблона…")
        await client.navigate(f"{HTML_URL}#main-console")

        # 1. main-console.png (1240 x 820)
        print("[2/6] Скриншот main-console.png (1240x820)…")
        await client.set_viewport(1240, 820, 1.0)
        await client.evaluate("window.location.hash = 'main-console'; updateRoute();")
        await asyncio.sleep(0.3)
        await client.capture_screenshot(
            os.path.join(SCREENSHOTS_DIR, "main-console.png")
        )

        # 2. queue-hub.png (1240 x 820)
        print("[3/6] Скриншот queue-hub.png (1240x820)…")
        await client.set_viewport(1240, 820, 1.0)
        await client.evaluate("window.location.hash = 'queue'; updateRoute();")
        await asyncio.sleep(0.3)
        await client.capture_screenshot(
            os.path.join(SCREENSHOTS_DIR, "queue-hub.png")
        )

        # 3. settings-models.png (1240 x 820)
        print("[4/6] Скриншот settings-models.png (1240x820)…")
        await client.set_viewport(1240, 820, 1.0)
        await client.evaluate("window.location.hash = 'settings-models'; updateRoute();")
        await asyncio.sleep(0.3)
        await client.capture_screenshot(
            os.path.join(SCREENSHOTS_DIR, "settings-models.png")
        )

        # 4. transcript-view.png (2480 x 1640 retina)
        print("[5/6] Скриншот transcript-view.png (2480x1640 retina)…")
        await client.set_viewport(1240, 820, 2.0)
        await client.evaluate("window.location.hash = 'transcript'; updateRoute();")
        await asyncio.sleep(0.3)
        transcript_png = os.path.join(SCREENSHOTS_DIR, "transcript-view.png")
        await client.capture_screenshot(transcript_png)

        # Конвертация в WebP для лендинга (2160 x 1428)
        site_webp = os.path.join(ROOT_DIR, "site/assets/screenshots/transcript.webp")
        subprocess.run(
            [
                "ffmpeg",
                "-y",
                "-i",
                transcript_png,
                "-vf",
                "scale=2160:-1",
                "-q:v",
                "85",
                site_webp,
            ],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        print(f"site/assets/screenshots/transcript.webp обновлён: {os.path.getsize(site_webp)} байт")

        # Соцпревью og-image.png из docs/og-template.html
        og_html = f"file://{DOCS_DIR}/og-template.html"
        await client.navigate(og_html)
        await client.set_viewport(1200, 630, 1.0)
        await asyncio.sleep(0.3)
        og_site_path = os.path.join(ROOT_DIR, "site/assets/og-image.png")
        await client.capture_screenshot(og_site_path)
        print(f"site/assets/og-image.png обновлён: {os.path.getsize(og_site_path)} байт")

        # Возвращаем шаблон для GIF
        await client.navigate(f"{HTML_URL}#main-console")

        # 5. demo-walkthrough.gif (880 x 609, 28 кадров + 1 финальный)
        print("[6/6] Рендеринг demo-walkthrough.gif и screen-recording-demo.gif…")
        wt_durations = [
            1000,
            160,
            200,
            240,
            200,
            160,
            200,
            200,
            160,
            200,
            200,
            200,
            160,
            200,
            200,
            320,
            360,
            200,
            320,
            1200,
            800,
            800,
            800,
            1000,
            1200,
            900,
            900,
            800,
            40,
        ]

        with tempfile.TemporaryDirectory() as tmpdir:
            await client.set_viewport(880, 609, 1.0)
            for f in range(28):
                await client.evaluate(
                    f"window.location.hash = 'wt-{f}'; updateRoute();"
                )
                await asyncio.sleep(0.04)
                frame_path = os.path.join(tmpdir, f"wt_{f:02d}.png")
                await client.capture_screenshot(frame_path)

            # 29-й кадр - копия 27-го для задержки
            shutil.copyfile(
                os.path.join(tmpdir, "wt_27.png"), os.path.join(tmpdir, "wt_28.png")
            )

            # Создаём файл списка для ffmpeg concat
            concat_txt = os.path.join(tmpdir, "concat.txt")
            with open(concat_txt, "w") as cf:
                for idx, dur in enumerate(wt_durations):
                    cf.write(f"file 'wt_{idx:02d}.png'\n")
                    cf.write(f"duration {dur / 1000.0:.3f}\n")
                cf.write("file 'wt_28.png'\n")

            gif_path = os.path.join(SCREENSHOTS_DIR, "demo-walkthrough.gif")
            pal_path = os.path.join(tmpdir, "palette.png")

            # Двухпроходная палитра для максимального качества GIF
            subprocess.run(
                [
                    "ffmpeg",
                    "-y",
                    "-f",
                    "concat",
                    "-safe",
                    "0",
                    "-i",
                    concat_txt,
                    "-vf",
                    "fps=15,palettegen=stats_mode=diff",
                    pal_path,
                ],
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )

            subprocess.run(
                [
                    "ffmpeg",
                    "-y",
                    "-f",
                    "concat",
                    "-safe",
                    "0",
                    "-i",
                    concat_txt,
                    "-i",
                    pal_path,
                    "-lavfi",
                    "fps=15 [x]; [x][1:v] paletteuse=dither=bayer:bayer_scale=3",
                    gif_path,
                ],
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            print(f"demo-walkthrough.gif готов: {os.path.getsize(gif_path)} байт")

        # 5. screen-recording-demo.gif (880 x 520, 4 кадра)
        sc_durations = [1520, 1480, 2520, 40]
        with tempfile.TemporaryDirectory() as tmpdir:
            await client.set_viewport(880, 520, 1.0)
            for i, name in enumerate(["sc-1", "sc-2", "sc-3"]):
                await client.evaluate(
                    f"window.location.hash = '{name}'; updateRoute();"
                )
                await asyncio.sleep(0.05)
                frame_path = os.path.join(tmpdir, f"sc_{i}.png")
                await client.capture_screenshot(frame_path)

            shutil.copyfile(
                os.path.join(tmpdir, "sc_2.png"), os.path.join(tmpdir, "sc_3.png")
            )

            concat_txt = os.path.join(tmpdir, "concat_sc.txt")
            with open(concat_txt, "w") as cf:
                for idx, dur in enumerate(sc_durations):
                    cf.write(f"file 'sc_{idx}.png'\n")
                    cf.write(f"duration {dur / 1000.0:.3f}\n")
                cf.write("file 'sc_3.png'\n")

            gif_path = os.path.join(SCREENSHOTS_DIR, "screen-recording-demo.gif")
            pal_path = os.path.join(tmpdir, "palette.png")

            subprocess.run(
                [
                    "ffmpeg",
                    "-y",
                    "-f",
                    "concat",
                    "-safe",
                    "0",
                    "-i",
                    concat_txt,
                    "-vf",
                    "fps=15,palettegen=stats_mode=diff",
                    pal_path,
                ],
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )

            subprocess.run(
                [
                    "ffmpeg",
                    "-y",
                    "-f",
                    "concat",
                    "-safe",
                    "0",
                    "-i",
                    concat_txt,
                    "-i",
                    pal_path,
                    "-lavfi",
                    "fps=15 [x]; [x][1:v] paletteuse=dither=bayer:bayer_scale=3",
                    gif_path,
                ],
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            print(f"screen-recording-demo.gif готов: {os.path.getsize(gif_path)} байт")

    finally:
        await client.close()
        runner.stop()
        print("Готово!")


if __name__ == "__main__":
    asyncio.run(main())
