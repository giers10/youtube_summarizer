#!/usr/bin/env python3

import os
import sqlite3
import subprocess
import sys

DB_FILE = os.path.join(os.path.dirname(__file__), 'summaries.db')
TRANSLATE_SCRIPT = os.path.join(os.path.dirname(__file__), 'translate_summary.py')
MODEL = "mistral-small3.1:24b"

def get_entries_needing_translation(conn):
    cursor = conn.cursor()
    cursor.execute(
        "SELECT id, summary_en, summary_de, summary_jp FROM summaries"
    )
    return [
        (row[0], row[1], row[2], row[3])
        for row in cursor.fetchall()
        if row[1] and (not row[2] or not row[3])  # summary_en vorhanden, mind. eine Übersetzung fehlt
    ]

def translate(summary_text, lang):
    # Schreibe summary_text temporär in Datei
    import tempfile
    with tempfile.NamedTemporaryFile('w+', delete=False, suffix='.txt', encoding='utf-8') as f:
        f.write(summary_text)
        tmp_summary_path = f.name
    try:
        # Führe das Übersetzungsskript aus
        cmd = [
            sys.executable,  # benutzt aktuelles Python
            TRANSLATE_SCRIPT,
            "--summary-file", tmp_summary_path,
            "--lang", lang,
            "--model", MODEL,
        ]
        print(f"[{lang}] Translating with: {' '.join(cmd)}")
        result = subprocess.run(cmd, capture_output=True, text=True, check=True)
        translation = result.stdout.strip()
        return translation
    finally:
        os.remove(tmp_summary_path)

def main():
    conn = sqlite3.connect(DB_FILE)
    cursor = conn.cursor()
    entries = get_entries_needing_translation(conn)
    print(f"Found {len(entries)} entries needing translation.")
    for entry_id, summary_en, summary_de, summary_jp in entries:
        updated = False
        if not summary_de:
            print(f"Translating to DE for entry id {entry_id}…")
            try:
                translation = translate(summary_en, "de")
                cursor.execute("UPDATE summaries SET summary_de = ? WHERE id = ?", (translation, entry_id))
                updated = True
            except Exception as e:
                print(f"Failed to translate DE for id {entry_id}: {e}")
        if not summary_jp:
            print(f"Translating to JP for entry id {entry_id}…")
            try:
                translation = translate(summary_en, "jp")
                cursor.execute("UPDATE summaries SET summary_jp = ? WHERE id = ?", (translation, entry_id))
                updated = True
            except Exception as e:
                print(f"Failed to translate JP for id {entry_id}: {e}")
        if updated:
            conn.commit()
    conn.close()
    print("Done.")

if __name__ == "__main__":
    main()