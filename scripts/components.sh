#!/usr/bin/env bash
# Проверенные компоненты: линт, мониторинг апстримов, проверка ffmpeg и моделей.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
pins="$root/src-tauri/crates/mr-app/components-pins.json"
cache="${PODTEKST_COMPONENTS_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/podtekst-components}"

usage() {
    cat <<'EOF'
usage: scripts/components.sh <команда>

  lint            оффлайн-линт манифеста моделей и пинов ffmpeg
  monitor [args]  апстримы компонентов: Markdown, --json, --offline
  verify-ffmpeg   скачать пин, сверить sha256, версию, архитектуру и энкодеры
  verify-models   живой ONNX-прогон (нужны модели, REQUIRE_REAL=1)
  report          линт + мониторинг + ffmpeg в runs/components/<дата>-<commit>/
EOF
}

host_platform() {
    case "$(uname -s)" in
        Linux) host_os=linux ;;
        Darwin) host_os=macos ;;
        MINGW*|MSYS*|CYGWIN*) host_os=windows ;;
        *) echo "components: ОС $(uname -s) не поддерживается" >&2; return 1 ;;
    esac
    case "$(uname -m)" in
        x86_64|amd64|AMD64) host_arch=x86_64 ;;
        aarch64|arm64|ARM64) host_arch=aarch64 ;;
        *) echo "components: архитектура $(uname -m) не поддерживается" >&2; return 1 ;;
    esac
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

run_limited() {
    # macOS без GNU timeout: -version и -encoders завершаются сами.
    if command -v timeout >/dev/null 2>&1; then
        timeout 15 "$@"
    else
        "$@"
    fi
}

pin_rows() {
    python3 - "$pins" "$host_os" "$host_arch" <<'PY'
import json
import sys
from pathlib import Path

path, os_name, arch = Path(sys.argv[1]), sys.argv[2], sys.argv[3]
try:
    data = json.loads(path.read_text(encoding="utf-8"))
except OSError as error:
    sys.exit(f"components: не читается {path}: {error}")

found = [p for p in data.get("packages", []) if p.get("os") == os_name and p.get("arch") == arch]
if not found:
    sys.exit(3)
for package in found:
    print(
        "\t".join(
            [
                package["version"],
                package["format"],
                package.get("sha256") or "-",
                package["url"],
                ",".join(package["provides"]),
            ]
        )
    )
PY
}

extract_archive() {
    python3 - "$1" "$2" <<'PY'
import shutil
import sys
import tarfile
import zipfile
from pathlib import Path

archive, dest = Path(sys.argv[1]), Path(sys.argv[2])
shutil.rmtree(dest, ignore_errors=True)
dest.mkdir(parents=True)


def unsafe(name):
    return Path(name).is_absolute() or ".." in Path(name).parts


if zipfile.is_zipfile(archive):
    with zipfile.ZipFile(archive) as bundle:
        for info in bundle.infolist():
            mode = info.external_attr >> 16
            if unsafe(info.filename) or (mode & 0o170000) == 0o120000:
                sys.exit(f"verify-ffmpeg: небезопасный элемент архива: {info.filename}")
        bundle.extractall(dest)
    sys.exit(0)

with tarfile.open(archive) as bundle:
    for member in bundle.getmembers():
        if unsafe(member.name) or member.issym() or member.islnk():
            sys.exit(f"verify-ffmpeg: небезопасный элемент архива: {member.name}")
    try:
        bundle.extractall(dest, filter="data")
    except TypeError:
        bundle.extractall(dest)
PY
}

check_arch() {
    python3 - "$1" "$host_arch" <<'PY'
import struct
import sys
from pathlib import Path

path, expected = Path(sys.argv[1]), sys.argv[2]
head = path.read_bytes()[:4096]
machines = {
    0x3E: "x86_64",
    0xB7: "aarch64",
    0x01000007: "x86_64",
    0x0100000C: "aarch64",
    0x8664: "x86_64",
    0xAA64: "aarch64",
}


def detected():
    if head[:4] == b"\x7fELF":
        return machines.get(struct.unpack_from("<H", head, 18)[0])
    if head[:4] == b"\xcf\xfa\xed\xfe":
        return machines.get(struct.unpack_from("<I", head, 4)[0])
    if head[:4] in (b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca"):
        return "fat"
    if head[:2] == b"MZ" and len(head) >= 0x40:
        offset = struct.unpack_from("<I", head, 0x3C)[0]
        if offset + 6 <= len(head) and head[offset : offset + 4] == b"PE\0\0":
            return machines.get(struct.unpack_from("<H", head, offset + 4)[0])
    return None


found = detected()
if found is None:
    sys.exit(f"verify-ffmpeg: не распознан формат бинарника: {path}")
if found != "fat" and found != expected:
    sys.exit(f"verify-ffmpeg: {path}: архитектура {found}, ожидалась {expected}")
print(f"  архитектура: {found}")
PY
}

check_version() {
    local binary="$1" expected="$2" output
    output="$(run_limited "$binary" -version 2>&1 | head -n1)"
    case "$output" in
        *"$expected"*) echo "  версия: $output" ;;
        *)
            echo "verify-ffmpeg: $binary: ожидалась $expected, получено: $output" >&2
            return 1
            ;;
    esac
}

check_encoders() {
    local binary="$1" output count
    output="$(run_limited "$binary" -encoders 2>&1)"
    count="$(printf '%s\n' "$output" | grep -cE '^ [VAS][A-Z.]{5} ' || true)"
    if [ "$count" -lt 20 ]; then
        echo "verify-ffmpeg: $binary: энкодеров $count, ожидалось не меньше 20" >&2
        return 1
    fi
    echo "  энкодеров: $count"
}

verify_package() {
    local version="$1" format="$2" sha="$3" url="$4" provides="$5"
    local name="${url##*/}" dir archive unpacked binary binary_name stamp="" size actual
    name="${name%%\?*}"
    dir="$cache/$host_os-$host_arch"
    if [ "$sha" = "-" ]; then
        stamp="-$(date +%Y%m%d)"
    fi
    archive="$dir/$version-$name$stamp"
    unpacked="$dir/unpacked-$version-$name"
    mkdir -p "$dir"

    if [ ! -f "$archive" ]; then
        echo "Скачивание $url"
        if ! curl -fL --retry 3 --connect-timeout 20 -o "$archive.part" "$url"; then
            rm -f "$archive.part"
            return 1
        fi
        mv "$archive.part" "$archive"
    fi
    if [ "$sha" != "-" ]; then
        actual="$(sha256_of "$archive")"
        if [ "$actual" != "$sha" ]; then
            echo "verify-ffmpeg: $name: sha256 $actual != $sha" >&2
            rm -f "$archive"
            return 1
        fi
    else
        size="$(wc -c < "$archive")"
        if [ "$size" -lt 10000000 ]; then
            echo "verify-ffmpeg: $name: размер $size байт меньше 10 МБ" >&2
            rm -f "$archive"
            return 1
        fi
    fi

    extract_archive "$archive" "$unpacked" || return 1
    while IFS= read -r binary_name; do
        binary="$(find "$unpacked" -type f \( -name "$binary_name" -o -name "$binary_name.exe" \) -print -quit)"
        if [ -z "$binary" ]; then
            echo "verify-ffmpeg: в $name нет $binary_name" >&2
            return 1
        fi
        chmod +x "$binary"
        echo "$binary_name ($format, $version)"
        check_arch "$binary" || return 1
        check_version "$binary" "$version" || return 1
        check_encoders "$binary" || return 1
    done < <(tr ',' '\n' <<< "$provides")
}

cmd_verify_ffmpeg() {
    if ! host_platform; then
        echo "verify-ffmpeg: пин для этой платформы не проверяется — пропуск"
        return 0
    fi
    if [ ! -f "$pins" ]; then
        echo "verify-ffmpeg: нет $pins" >&2
        return 1
    fi

    local rows status=0
    rows="$(pin_rows)" || status=$?
    if [ "$status" -eq 3 ]; then
        echo "verify-ffmpeg: пина для $host_os/$host_arch нет — пропуск"
        return 0
    fi
    if [ "$status" -ne 0 ]; then
        return "$status"
    fi

    local version format sha url provides
    while IFS=$'\t' read -r version format sha url provides; do
        verify_package "$version" "$format" "$sha" "$url" "$provides" || return 1
    done <<< "$rows"
    echo "verify-ffmpeg: OK ($host_os/$host_arch)"
}

cmd_report() {
    local commit stamp dir status_lint=0 status_monitor=0 status_verify=0
    commit="$(git -C "$root" rev-parse --short HEAD)"
    stamp="$(date +%Y-%m-%d)"
    dir="$root/runs/components/$stamp-$commit"
    mkdir -p "$dir"
    if ! git -C "$root" check-ignore -q "$dir"; then
        echo "components: runs/ не в .gitignore — артефакты прогона не коммитить" >&2
    fi

    echo "lint: $dir/lint.txt"
    python3 "$root/scripts/components_lint.py" > "$dir/lint.txt" 2>&1 || status_lint=$?
    echo "monitor: $dir/monitor.json"
    python3 "$root/scripts/components_monitor.py" --json > "$dir/monitor.json" 2> "$dir/monitor.err" || status_monitor=$?
    echo "verify-ffmpeg: $dir/verify-ffmpeg.txt"
    cmd_verify_ffmpeg > "$dir/verify-ffmpeg.txt" 2>&1 || status_verify=$?

    python3 - "$dir" "$root" "$commit" "$status_lint" "$status_monitor" "$status_verify" <<'PY'
import json
import platform
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

directory, root = Path(sys.argv[1]), Path(sys.argv[2])
commit, lint, monitor, verify = sys.argv[3:7]


def read_json(path):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None


def read_text(path):
    try:
        return path.read_text(encoding="utf-8").strip()
    except OSError:
        return ""


manifest = read_json(root / "src-tauri/crates/mr-models/models-manifest.json") or {}
pins = read_json(root / "src-tauri/crates/mr-app/components-pins.json") or {}
monitor_data = read_json(directory / "monitor.json") or {}
cargo = read_text(root / "src-tauri/crates/mr-app/Cargo.toml")
app_version = re.search(r'^version\s*=\s*"([^"]+)"', cargo, re.MULTILINE)
dirty = subprocess.run(
    ["git", "-C", str(root), "status", "--porcelain"], capture_output=True, text=True
).stdout.strip()

models = {
    model.get("id", "?"): model.get("version")
    for model in manifest.get("models", [])
    if isinstance(model, dict)
}
data = {
    "generated_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
    "commit": commit,
    "dirty": bool(dirty),
    "os": platform.system().lower(),
    "arch": platform.machine(),
    "app_version": app_version.group(1) if app_version else None,
    "models": models,
    "pins": [
        {key: package.get(key) for key in ("os", "arch", "version", "source")}
        for package in pins.get("packages", [])
        if isinstance(package, dict)
    ],
    "checks": {
        "lint": int(lint),
        "monitor": int(monitor),
        "verify_ffmpeg": int(verify),
    },
    "monitor": monitor_data.get("summary", {}),
}
(directory / "components.json").write_text(
    json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
)

lines = [
    f"# Компоненты: {directory.name}",
    "",
    f"- Commit: `{commit}`" + (" (грязное дерево)" if data["dirty"] else ""),
    f"- ОС: {data['os']} / {data['arch']}",
    f"- Версия приложения: {data['app_version']}",
    f"- Моделей в манифесте: {len(models)}",
    f"- Пинов ffmpeg: {len(data['pins'])}",
    "",
    "| Проверка | Код |",
    "|---|---|",
    f"| lint | {data['checks']['lint'] or 'ok'} |",
    f"| monitor | {data['checks']['monitor'] or 'ok'} |",
    f"| verify-ffmpeg | {data['checks']['verify_ffmpeg'] or 'ok'} |",
    "",
    "verify-models не запускался: нужны модели в `MR_MODELS_DIR`.",
    "",
]

rows = monitor_data.get("rows", [])
summary = monitor_data.get("summary", {})
lines += ["## Мониторинг", ""]
if rows:
    lines += ["| Компонент | Сейчас | Апстрим | Статус | Ссылка |", "|---|---|---|---|---|"]
    lines += [
        f"| {row['component']} | {row['current']} | {row['upstream']} | {row['status']} | {row['link']} |"
        for row in rows
    ]
    lines += [
        "",
        f"Итог: обновлений — {summary.get('updates', 0)}, ошибок сети — "
        f"{summary.get('network_errors', 0)}, не проверено — {summary.get('unchecked', 0)}.",
    ]
else:
    lines.append("Мониторинг не выполнялся.")
if not monitor_data.get("pins_present", False):
    lines += ["", "Пины ffmpeg отсутствуют: в мониторинге нет строк ffmpeg."]
lines += ["", "## Линт", "", "```text", read_text(directory / "lint.txt") or "—", "```", ""]
lines += [
    "## verify-ffmpeg",
    "",
    "```text",
    read_text(directory / "verify-ffmpeg.txt") or "—",
    "```",
    "",
]
monitor_errors = read_text(directory / "monitor.err")
if monitor_errors:
    lines += ["## stderr мониторинга", "", "```text", monitor_errors, "```", ""]
(directory / "report.md").write_text("\n".join(lines), encoding="utf-8")
print(f"report: {directory / 'report.md'}")
PY

    if [ "$status_lint" -ne 0 ] || [ "$status_monitor" -ne 0 ] || [ "$status_verify" -ne 0 ]; then
        echo "components: проверки с ошибками (lint=$status_lint, monitor=$status_monitor, verify-ffmpeg=$status_verify)" >&2
        return 1
    fi
    echo "components: OK"
}

command="${1:-}"
if [ $# -gt 0 ]; then
    shift
fi

case "$command" in
    lint) python3 "$root/scripts/components_lint.py" ;;
    monitor) python3 "$root/scripts/components_monitor.py" "$@" ;;
    verify-ffmpeg) cmd_verify_ffmpeg ;;
    verify-models)
        models_dir="${MR_MODELS_DIR:-}"
        if [ -z "$models_dir" ] || [ ! -d "$models_dir" ]; then
            echo "verify-models: задайте MR_MODELS_DIR с моделями" >&2
            exit 2
        fi
        if ! find "$models_dir" -name '*.onnx' -print -quit | grep -q .; then
            echo "verify-models: в $models_dir нет .onnx" >&2
            exit 2
        fi
        REQUIRE_REAL=1 cargo test \
            --manifest-path "$root/src-tauri/Cargo.toml" \
            -p mr-ml --features onnx --test onnx_real -- --nocapture
        ;;
    report) cmd_report ;;
    *)
        usage
        exit 2
        ;;
esac
