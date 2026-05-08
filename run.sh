#!/usr/bin/env bash
set -e

cd "$(dirname "$0")"

# 1. Python venv einrichten
GREEN="\033[0;32m"
CYAN="\033[0;36m"
NC="\033[0m" # No Color
echo -e "${CYAN}1. Python venv einrichten …${NC}"
if [ ! -d "venv" ]; then
    python3 -m venv venv
fi

# 2. venv aktivieren
echo -e "${CYAN}2. Aktiviere venv …${NC}"
source venv/bin/activate

# 3. Python-Abhängigkeiten installieren
echo -e "${CYAN}3. Python-Abhängigkeiten installieren …${NC}"
pip install --upgrade pip
pip install -r requirements.txt
pip install --upgrade yt-dlp

# 4. Tauri dev resources writable halten
echo -e "${CYAN}4. Tauri dev resources vorbereiten …${NC}"
find src-tauri/resources src-tauri/target/debug \
    \( -path "*/backend/*" -o -path "*/ffmpeg/*" \) \
    -type f -exec chmod u+w {} + 2>/dev/null || true

# 5. Tauri App starten
echo -e "${CYAN}5. Starte die Tauri App …${NC}"
cargo run --manifest-path src-tauri/Cargo.toml
