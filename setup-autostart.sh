#!/usr/bin/env bash
# Скрипт управления автозапуском "Записи встреч" в Ubuntu/Linux
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
AUTOSTART_DIR="$HOME/.config/autostart"
TARGET_FILE="$AUTOSTART_DIR/meeting-recorder.desktop"

case "$1" in
    enable|"")
        mkdir -p "$AUTOSTART_DIR"
        cat <<EOF > "$TARGET_FILE"
[Desktop Entry]
Version=1.0
Type=Application
Name=Запись встреч (Фоновый режим)
GenericName=Фоновая аудиозапись встреч и звонков
Comment=Автозапуск в трее для быстрой записи микрофона и звука приложений
Exec="$SCRIPT_DIR/run.sh" --tray
Icon=$SCRIPT_DIR/icon.svg
Terminal=false
Categories=AudioVideo;Audio;Recorder;Utility;
Keywords=audio;record;meeting;telegram;yandex;микрофон;запись;встречи;звонки;
StartupNotify=false
X-GNOME-Autostart-enabled=true
X-GNOME-Autostart-Delay=2
EOF
        chmod +x "$TARGET_FILE" 2>/dev/null || true
        echo "✓ Автозапуск включен: создан файл $TARGET_FILE"
        echo "  Приложение будет запускаться в трее при каждом входе в систему."
        ;;
    install-desktop)
        APP_DESKTOP_DIR="$HOME/.local/share/applications"
        APP_TARGET_FILE="$APP_DESKTOP_DIR/meeting-recorder.desktop"
        mkdir -p "$APP_DESKTOP_DIR"
        cat <<EOF > "$APP_TARGET_FILE"
[Desktop Entry]
Version=1.0
Type=Application
Name=Запись встреч
GenericName=Аудиозапись встреч и звонков
Comment=Запись звука с микрофона и приложений (Яндекс Браузер, Telegram, звонки)
Exec="$SCRIPT_DIR/run.sh"
Icon=$SCRIPT_DIR/icon.svg
Terminal=false
Categories=AudioVideo;Audio;Recorder;Utility;
Keywords=audio;record;meeting;telegram;yandex;микрофон;запись;встречи;звонки;
StartupNotify=true
StartupWMClass=meeting-recorder
Actions=ToggleRecord;ShowRecorder;

[Desktop Action ToggleRecord]
Name=Старт / Стоп записи
Exec="$SCRIPT_DIR/run.sh" --toggle

[Desktop Action ShowRecorder]
Name=Открыть окно
Exec="$SCRIPT_DIR/run.sh" --show
EOF
        chmod +x "$APP_TARGET_FILE" 2>/dev/null || true
        echo "✓ Ярлык приложения установлен: $APP_TARGET_FILE"
        ;;
    disable)
        if [ -f "$TARGET_FILE" ]; then
            rm -f "$TARGET_FILE"
            echo "✓ Автозапуск выключен: удален файл $TARGET_FILE"
        else
            echo "Автозапуск уже был выключен."
        fi
        ;;
    status)
        if [ -f "$TARGET_FILE" ]; then
            echo "Статус автозапуска: ВКЛЮЧЕН ($TARGET_FILE)"
        else
            echo "Статус автозапуска: ВЫКЛЮЧЕН"
        fi
        ;;
    *)
        echo "Использование: $0 [enable|disable|status|install-desktop]"
        exit 1
        ;;
esac
