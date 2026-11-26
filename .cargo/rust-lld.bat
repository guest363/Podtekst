@echo off
setlocal
set "RUSTC_BIN=%RUSTC%"
if "%RUSTC_BIN%"=="" set "RUSTC_BIN=rustc"
for /f "usebackq delims=" %%i in (`"%RUSTC_BIN%" --print sysroot`) do set "RUST_SYSROOT=%%i"
set "RUST_LLD=%RUST_SYSROOT%\lib\rustlib\x86_64-pc-windows-msvc\bin\rust-lld.exe"
if not exist "%RUST_LLD%" (
  echo rust-lld.exe missing: "%RUST_LLD%" 1>&2
  exit /b 1
)
if "%~1"=="-flavor" (
  "%RUST_LLD%" %*
) else (
  "%RUST_LLD%" -flavor link %*
)
exit /b %ERRORLEVEL%
