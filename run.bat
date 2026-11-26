@echo off
rem Подтекст (Podtekst) — запуск нативного ядра.
setlocal
cd /d "%~dp0"
where cargo >nul 2>nul
if %errorlevel% neq 0 (
    echo [ОШИБКА] Установите Rust с https://rustup.rs
    pause
    exit /b 1
)
set "FEATS=--features tauri-ui"
rem %* может содержать кавычки и &, поэтому ищем флаг в одном аргументе без echo.
for %%a in (%*) do (
    if /I "%%~a"=="--web" set "FEATS="
)
rem --bin podtekst пропускает сборку мобильных staticlib/cdylib mr_app_lib.
cargo build --release --manifest-path src-tauri\Cargo.toml -p mr-app --bin podtekst %FEATS%
if errorlevel 1 (
    echo [ОШИБКА] Сборка не удалась — запуск отменён
    pause
    exit /b 1
)
rem Все сборки проекта складываются в одну папку dist\ в корне репозитория.
if not exist dist mkdir dist
copy /y src-tauri\target\release\podtekst.exe dist\podtekst-windows.exe >nul
if errorlevel 1 (
    echo [ОШИБКА] Не удалось скопировать podtekst.exe в dist
    pause
    exit /b 1
)
src-tauri\target\release\podtekst.exe %*
