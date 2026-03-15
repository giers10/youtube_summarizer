@echo off
setlocal

REM 1. Prüfen, ob venv existiert, sonst erstellen
if not exist venv (
    echo Erstelle Python venv...
    python -m venv venv
)

REM 2. venv aktivieren
echo Aktiviere venv...
call venv\Scripts\activate

REM 3. Python-Abhängigkeiten installieren
echo Installiere Python requirements...
pip install --upgrade pip
pip install -r requirements.txt

REM 4. Tauri App starten
echo Starte die Tauri App...
cargo run --manifest-path src-tauri/Cargo.toml

REM 6. Deaktivieren (optional)
deactivate

endlocal
pause
