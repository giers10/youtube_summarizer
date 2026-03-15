#!/usr/bin/env python3
"""
translate_summary.py

Usage:
    python3 translate_summary.py --summary-file <file> --lang <de|jp> [--model <model>] [--output-file <file>]

Arguments:
    --summary-file   Path to the file containing the English summary text.
    --lang           Target language ('de' for German, 'jp' for Japanese).
    --model          (Optional) Ollama model name, defaults to mistral:latest.
    --output-file    (Optional) Where to write translated summary as plain text.

Example:
    python3 translate_summary.py --summary-file summary.txt --lang de --model mistral:latest
"""

import sys
import argparse
import json
import requests

LANG_MAP = {
    "de": "German",
    "jp": "Japanese"
}

def translate_summary_text(summary_text, target_language, model="mistral:latest"):
    if target_language not in LANG_MAP:
        raise ValueError("Supported languages: de (German), jp (Japanese)")
    prompt = (
        f"Translate the following summary into {LANG_MAP[target_language]}. Only output the translated summary, "
        "no explanation or intro. If it's already in the target language, do nothing but repeat it.\n\n"
        f"Summary:\n{summary_text}\n\nTranslation:"
    )
    payload = {
        "model": model,
        "messages": [
            {"role": "system", "content": f"You are an expert translator proficient in {LANG_MAP[target_language]} and English."},
            {"role": "user", "content": prompt}
        ],
        "stream": False
    }
    resp = requests.post("http://localhost:11434/api/chat", json=payload)
    resp.raise_for_status()
    data = resp.json()
    return data.get("message", {}).get("content", "").strip()


def translate_summary_file(summary_file, target_language, model="mistral:latest"):
    with open(summary_file, "r", encoding="utf-8") as f:
        summary_text = f.read().strip()
    if not summary_text:
        raise ValueError("Empty summary text!")
    return translate_summary_text(summary_text, target_language, model)

def main():
    parser = argparse.ArgumentParser(description="Translate summary using Ollama")
    parser.add_argument("--summary-file", required=True, help="Path to file with English summary text")
    parser.add_argument("--lang", required=True, choices=["de", "jp"], help="Target language: 'de' or 'jp'")
    parser.add_argument("--model", default="mistral:latest", help="Ollama model to use")
    parser.add_argument("--output-file", help="Output file for translated summary")
    args = parser.parse_args()

    # Read summary
    try:
        translation = translate_summary_file(args.summary_file, args.lang, args.model)
    except Exception as e:
        print(f"Translation failed: {e}", file=sys.stderr)
        sys.exit(2)

    # Output result
    if args.output_file:
        with open(args.output_file, "w", encoding="utf-8") as f:
            f.write(translation)
    else:
        print(translation)

if __name__ == "__main__":
    main()
