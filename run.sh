#!/usr/bin/env bash
set -e

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

# 4. Tauri App starten
echo -e "${CYAN}4. Starte die Tauri App …${NC}"
cargo run --manifest-path src-tauri/Cargo.toml
