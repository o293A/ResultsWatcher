@echo off
REM Builds ResultsWatcher.exe (needs Rust: https://rustup.rs). The result is a single self-contained file.
cargo build --release
if errorlevel 1 (echo Build failed & pause & exit /b 1)
copy /Y target\release\ResultsWatcher.exe ResultsWatcher.exe >nul
if errorlevel 1 (echo Could not copy the exe & pause & exit /b 1)

REM Permanently delete the target folder (no recycle bin)
if exist target rmdir /s /q target

echo.
echo Done: ResultsWatcher.exe
echo You can put ResultsWatcher.exe anywhere you want: it is the whole app, nothing else is needed.
echo (screens\ and watcher.log are created next to it on first launch; config.toml is optional)
pause
