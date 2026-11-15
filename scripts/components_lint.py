#!/usr/bin/env python3
"""Оффлайн-линт проверенных компонентов: манифест моделей и пины ffmpeg.

Гейт CI и `scripts/components.sh lint`. Сеть не нужна.
"""

import argparse
import datetime
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "src-tauri/crates/mr-models/models-manifest.json"
PINS = ROOT / "src-tauri/crates/mr-app/components-pins.json"

SHA256 = re.compile(r"[0-9a-f]{64}")
DRIVE = re.compile(r"[A-Za-z]:")
KINDS = ("hf", "github")
OS_VALUES = ("windows", "macos", "linux")
ARCH_VALUES = ("x86_64", "aarch64")
FORMATS = ("zip", "tar.xz")
REQUIRED_TARGETS = (
    ("windows", "x86_64"),
    ("macos", "aarch64"),
    ("linux", "x86_64"),
    ("linux", "aarch64"),
)
WINDOWS_RESERVED = {
    "CON",
    "PRN",
    "AUX",
    "NUL",
    *(f"COM{index}" for index in range(1, 10)),
    *(f"LPT{index}" for index in range(1, 10)),
}
WINDOWS_FORBIDDEN = set('<>:"|?*')


def is_text(value):
    return isinstance(value, str) and value != "" and value.strip() == value


def is_sha256(value):
    return is_text(value) and SHA256.fullmatch(value) is not None and len(set(value)) > 1


def load(path, errors, label):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        errors.append(f"{label}: не читается {path}: {error}")
    except json.JSONDecodeError as error:
        errors.append(f"{label}: невалидный JSON в {path}: {error}")
    return None


def filename_problem(name):
    if not isinstance(name, str) or name == "":
        return "пустое имя"
    if name.strip() != name:
        return "пробел в начале или в конце"
    if "/" in name or "\\" in name:
        return "содержит разделитель пути"
    if ".." in name:
        return "содержит «..»"
    if Path(name).is_absolute() or DRIVE.match(name):
        return "абсолютный путь"
    if any(symbol in WINDOWS_FORBIDDEN or ord(symbol) < 32 for symbol in name):
        return "недопустимые для Windows символы"
    if name.endswith((".", " ")):
        return "точка или пробел в конце недопустимы в Windows"
    if name.split(".")[0].upper() in WINDOWS_RESERVED:
        return "зарезервированное имя Windows"
    return None


def url_problems(url, kind, revision):
    problems = []
    if not is_text(url) or not url.startswith("https://"):
        problems.append("URL должен быть https")
    if "/resolve/main/" in url or "/resolve/master/" in url:
        problems.append("ревизия не закреплена: resolve/main")
    elif is_text(kind) and is_text(revision):
        if kind == "hf" and f"/resolve/{revision}/" not in url:
            problems.append(f"URL не содержит /resolve/{revision}/")
        if kind != "hf" and f"/{revision}/" not in url:
            problems.append(f"URL не содержит ревизию {revision}")
    return problems


def check_models(manifest, errors):
    models = manifest.get("models") if isinstance(manifest, dict) else None
    if not isinstance(models, list) or not models:
        errors.append("models-manifest.json: models[] пуст или отсутствует")
        return
    updated = manifest.get("updated_at")
    if not is_text(updated):
        errors.append("models-manifest.json: updated_at пуст")
    else:
        try:
            datetime.datetime.fromisoformat(updated.replace("Z", "+00:00"))
        except ValueError:
            errors.append(f"models-manifest.json: updated_at не ISO 8601: {updated!r}")
    ids = {}
    names = {}
    lower_names = {}
    for index, model in enumerate(models):
        where = f"models[{index}]"
        if not isinstance(model, dict):
            errors.append(f"{where}: элемент не объект")
            continue
        model_id = model.get("id")
        if not is_text(model_id):
            errors.append(f"{where}: не заполнен id")
            model_id = f"#{index}"
        else:
            problem = filename_problem(model_id)
            if problem:
                errors.append(f"{where}: id {model_id!r}: {problem}")
            if model_id.endswith(".new") or ".old-" in model_id:
                errors.append(f"{where}: id {model_id!r}: служебный суффикс каталога")
            if model_id in ids:
                errors.append(f"{where}: id {model_id} повторяется, первый в models[{ids[model_id]}]")
            else:
                ids[model_id] = index
        where = f"модель {model_id}"

        order = model.get("order")
        if not isinstance(order, int) or isinstance(order, bool) or order <= 0:
            errors.append(f"{where}: order должен быть целым > 0, сейчас {order!r}")

        upstream = model.get("upstream")
        kind = repo = revision = None
        if not isinstance(upstream, dict):
            errors.append(f"{where}: нет upstream (kind, repo, revision, version)")
        else:
            kind, repo, revision = (
                upstream.get("kind"),
                upstream.get("repo"),
                upstream.get("revision"),
            )
            for field in ("kind", "repo", "revision", "version"):
                if not is_text(upstream.get(field)):
                    errors.append(f"{where}: upstream.{field} пуст")
            if is_text(kind) and kind not in KINDS:
                errors.append(f"{where}: upstream.kind={kind!r}, допустимо {', '.join(KINDS)}")
            if revision in ("main", "master"):
                errors.append(f"{where}: upstream.revision={revision!r} не закрепляет версию")

        entries = model.get("files")
        if not isinstance(entries, list) or not entries:
            errors.append(f"{where}: files[] пуст или отсутствует")
            continue
        for file_index, entry in enumerate(entries):
            file_where = f"{where}, файл #{file_index + 1}"
            if not isinstance(entry, dict):
                errors.append(f"{file_where}: не объект")
                continue
            name = entry.get("filename")
            problem = filename_problem(name)
            if problem:
                errors.append(f"{file_where}: имя {name!r}: {problem}")
            else:
                file_where = f"{where}, файл {name}"
                if name in names:
                    errors.append(f"{file_where}: имя повторяется, первый раз в модели {names[name]}")
                elif name.lower() in lower_names:
                    errors.append(
                        f"{file_where}: коллизия регистра с файлом модели {lower_names[name.lower()]}"
                    )
                else:
                    names[name] = model_id
                    lower_names[name.lower()] = model_id

            if not is_sha256(entry.get("sha256")):
                errors.append(f"{file_where}: sha256 должен быть 64 hex-символа без placeholder")
            size = entry.get("size_bytes")
            if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
                errors.append(f"{file_where}: size_bytes должен быть целым > 0")

            urls = []
            main_url = entry.get("url_hf")
            if not is_text(main_url):
                errors.append(f"{file_where}: нет url_hf")
            else:
                urls.append(main_url)
            mirrors = entry.get("mirrors") or []
            if not isinstance(mirrors, list):
                errors.append(f"{file_where}: mirrors должен быть массивом")
                mirrors = []
            urls.extend(url for url in mirrors if is_text(url))
            for url in urls:
                for problem in url_problems(url, kind, revision):
                    errors.append(f"{file_where}: {url}: {problem}")


def check_pins(pins, errors):
    if not isinstance(pins, dict):
        errors.append("components-pins.json: корень должен быть объектом")
        return
    if pins.get("schema") != 1:
        errors.append(f"components-pins.json: schema должен быть 1, сейчас {pins.get('schema')!r}")
    packages = pins.get("packages")
    if not isinstance(packages, list) or not packages:
        errors.append("components-pins.json: packages[] пуст или отсутствует")
        return

    targets = set()
    provides_by_target = {}
    versions_by_target = {}
    for index, package in enumerate(packages):
        where = f"пин #{index + 1}"
        if not isinstance(package, dict):
            errors.append(f"{where}: элемент не объект")
            continue

        os_name, arch = package.get("os"), package.get("arch")
        if os_name not in OS_VALUES:
            errors.append(f"{where}: os={os_name!r}, допустимо {', '.join(OS_VALUES)}")
        if arch not in ARCH_VALUES:
            errors.append(f"{where}: arch={arch!r}, допустимо {', '.join(ARCH_VALUES)}")
        if (os_name, arch) == ("macos", "x86_64"):
            errors.append(f"{where}: macOS x86_64 не поддерживается")
        target = f"{os_name}/{arch}"
        where = f"пин {target}"
        if os_name in OS_VALUES and arch in ARCH_VALUES:
            targets.add((os_name, arch))

        if not is_text(package.get("version")):
            errors.append(f"{where}: version пуст")
        if not is_text(package.get("source")):
            errors.append(f"{where}: source пуст")
        if not is_text(package.get("url")) or not package["url"].startswith("https://"):
            errors.append(f"{where}: url должен быть https")
        if package.get("format") not in FORMATS:
            errors.append(f"{where}: format={package.get('format')!r}, допустимо {', '.join(FORMATS)}")

        sha = package.get("sha256")
        if sha is None:
            if package.get("source") != "johnvansickle":
                errors.append(f"{where}: sha256=null допустим только для источника johnvansickle")
            elif not is_text(package.get("version")):
                errors.append(f"{where}: пакет без sha256 обязан иметь version")
        elif not is_sha256(sha):
            errors.append(f"{where}: sha256 должен быть 64 hex-символа или null")

        if is_text(package.get("version")):
            versions_by_target.setdefault(target, set()).add(package["version"])

        names = package.get("provides")
        if not isinstance(names, list) or not names:
            errors.append(f"{where}: provides[] пуст или отсутствует")
            continue
        seen = provides_by_target.setdefault(f"{os_name}/{arch}", set())
        for name in names:
            if not is_text(name):
                errors.append(f"{where}: provides содержит пустое имя")
            elif name in seen:
                errors.append(f"{where}: provides дублирует {name} в рамках одного target")
            else:
                seen.add(name)

    for target, versions in versions_by_target.items():
        if len(versions) > 1:
            errors.append(
                f"components-pins.json: {target}: у пакетов разные версии: "
                + ", ".join(sorted(versions))
            )
    for target, names in provides_by_target.items():
        missing = {"ffmpeg", "ffprobe"} - names
        if missing:
            errors.append(f"components-pins.json: {target}: нет {', '.join(sorted(missing))}")

    for os_name, arch in REQUIRED_TARGETS:
        if (os_name, arch) not in targets:
            errors.append(f"components-pins.json: нет пина для {os_name}/{arch}")


def self_test():
    def model(**patch):
        entry = {
            "id": "m1",
            "order": 1,
            "upstream": {"kind": "hf", "repo": "o/r", "revision": "a" * 40, "version": "1.0"},
            "files": [
                {
                    "filename": "m.onnx",
                    "url_hf": "https://huggingface.co/o/r/resolve/" + "a" * 40 + "/m.onnx",
                    "size_bytes": 1,
                    "sha256": "b" * 63 + "c",
                }
            ],
        }
        entry.update(patch)
        return entry

    def pin(**patch):
        entry = {
            "os": "linux",
            "arch": "x86_64",
            "version": "1.0",
            "source": "johnvansickle",
            "provides": ["ffmpeg", "ffprobe"],
            "format": "tar.xz",
            "url": "https://example.com/a.tar.xz",
            "sha256": None,
        }
        entry.update(patch)
        return entry

    def hashed(**patch):
        base = {"source": "gyan", "format": "zip", "url": "https://example.com/a.zip"}
        base.update(patch)
        return pin(**base) | {"sha256": "c" * 63 + "d"}

    def pins(*packages):
        return {"schema": 1, "packages": list(packages)}

    def models(*entries):
        return {"updated_at": "2026-11-15T00:00:00", "models": list(entries)}

    full_pins = pins(
        hashed(os="windows", arch="x86_64"),
        hashed(os="macos", arch="aarch64", source="martin-riedl"),
        pin(),
        pin(arch="aarch64"),
    )
    bad_file = model(
        files=[
            {
                "filename": "m.onnx",
                "url_hf": "https://huggingface.co/o/r/resolve/" + "a" * 40 + "/m.onnx",
                "size_bytes": 1,
                "sha256": "b" * 63 + "c",
            },
            {
                "filename": "M.ONNX",
                "url_hf": "https://huggingface.co/o/r/resolve/" + "a" * 40 + "/M.ONNX",
                "size_bytes": 1,
                "sha256": "b" * 63 + "c",
            },
        ]
    )

    cases = [
        ("манифест без upstream", models(model(upstream=None)), "нет upstream"),
        ("order 0", models(model(order=0)), "order"),
        ("дата не ISO", models(model()) | {"updated_at": "15.11.2026"}, "ISO"),
        ("id с путём", models(model(id="../m")), "id"),
        ("id Windows", models(model(id="CON")), "Windows"),
        ("id staging", models(model(id="m.new")), "суффикс"),
        ("путь в имени", models(model(files=[{**model()["files"][0], "filename": "a/b.onnx"}])), "разделитель"),
        ("зарезервированное имя", models(model(files=[{**model()["files"][0], "filename": "CON.txt"}])), "Windows"),
        ("placeholder sha", models(model(files=[{**model()["files"][0], "sha256": "0" * 64}])), "placeholder"),
        (
            "url на main",
            models(
                model(
                    files=[
                        {
                            **model()["files"][0],
                            "url_hf": "https://huggingface.co/o/r/resolve/main/m.onnx",
                        }
                    ]
                )
            ),
            "main",
        ),
        ("коллизия регистра", models(bad_file), "регистра"),
        (
            "null sha не для johnvansickle",
            pins(full_pins["packages"][0] | {"sha256": None}),
            "johnvansickle",
        ),
        (
            "null sha без version",
            pins(*full_pins["packages"][:2], pin(version="")),
            "обязан иметь version",
        ),
        ("нет покрытия linux/aarch64", pins(*full_pins["packages"][:3]), "linux/aarch64"),
        (
            "дубль provides",
            pins(full_pins["packages"][0] | {"provides": ["ffmpeg", "ffmpeg"]}),
            "дублирует",
        ),
        (
            "нет ffprobe",
            pins(*full_pins["packages"][:3], pin(arch="aarch64", provides=["ffmpeg"])),
            "ffprobe",
        ),
        (
            "разные версии target",
            pins(*full_pins["packages"], pin(provides=["ffprobe"], version="2.0")),
            "разные версии",
        ),
    ]

    failed = False
    for label, document, needle in cases:
        errors = []
        if "models" in document:
            check_models(document, errors)
        else:
            check_pins(document, errors)
        if not any(needle in error for error in errors):
            print(f"self-test FAIL [{label}]: ожидалась ошибка про {needle!r}, получено {errors}")
            failed = True

    for label, document in (("валидный манифест", models(model())), ("валидные пины", full_pins)):
        errors = []
        if "models" in document:
            check_models(document, errors)
        else:
            check_pins(document, errors)
        if errors:
            print(f"self-test FAIL [{label}]: {errors}")
            failed = True

    if failed:
        return 1
    print("components-lint: self-test OK")
    return 0


def main():
    parser = argparse.ArgumentParser(description="Оффлайн-линт манифеста моделей и пинов ffmpeg")
    parser.add_argument("--self-test", action="store_true", help="прогнать встроенные примеры правил")
    args = parser.parse_args()
    if args.self_test:
        return self_test()

    errors = []
    manifest = load(MANIFEST, errors, "models-manifest.json")
    if manifest is not None:
        check_models(manifest, errors)
    if PINS.is_file():
        pins = load(PINS, errors, "components-pins.json")
        if pins is not None:
            check_pins(pins, errors)
    else:
        errors.append(f"components-pins.json: нет файла {PINS}")

    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"components-lint: ошибок {len(errors)}", file=sys.stderr)
        return 1
    print("components-lint: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
