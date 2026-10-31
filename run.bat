@echo off
rem Подтекст (Podtekst) — запуск нативного ядра.
cd /d "%~dp0"
where cargo >nul 2>nul
if %errorlevel% neq 0 (
    echo [ОШИБКА] Установите Rust с https://rustup.rs
    pause
    exit /b 1
)
set "FEATS=--features tauri-ui"
echo %* | find "--web" >nul && set "FEATS="
rem Cargo сам инкрементально пересобирает изменённые исходники (после git pull)
cargo build --release --manifest-path src-tauri\Cargo.toml -p mr-app %FEATS%
rem Все сборки проекта складываются в одну папку dist\ в корне репозитория.
if not exist dist mkdir dist
copy /y src-tauri\target\release\podtekst.exe dist\podtekst-windows.exe >nul
src-tauri\target\release\podtekst.exe %*
