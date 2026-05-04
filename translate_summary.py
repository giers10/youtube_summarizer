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
import math
import requests

LANG_MAP = {
    "de": "German",
    "jp": "Japanese"
}
OLLAMA_CHARS_PER_TOKEN = 3.5
OLLAMA_OUTPUT_TOKEN_BUDGET = 2048
OLLAMA_CONTEXT_BUCKETS = (4096, 8192, 16384, 32768, 65536)


def default_translation_prompt_template(target_language):
    if target_language not in LANG_MAP:
        raise ValueError("Supported languages: de (German), jp (Japanese)")
    return (
        f"Translate the following summary into {LANG_MAP[target_language]}. Only output the translated summary, "
        "no explanation or intro. If it's already in the target language, do nothing but repeat it.\n\n"
        "Summary:\n{summary}\n\nTranslation:"
    )


def render_translation_prompt(summary_text, target_language, prompt_template=None):
    template = (prompt_template or default_translation_prompt_template(target_language)).strip()
    prompt = (
        template
        .replace("{language}", LANG_MAP[target_language])
        .replace("{summary}", summary_text)
    )
    if "{summary}" not in template:
        prompt = f"{prompt}\n\nSummary:\n{summary_text}\n\nTranslation:"
    return prompt


def choose_ollama_num_ctx(prompt, output_budget=OLLAMA_OUTPUT_TOKEN_BUDGET):
    estimated_input_tokens = math.ceil(len(prompt) / OLLAMA_CHARS_PER_TOKEN)
    needed_tokens = estimated_input_tokens + output_budget
    for bucket in OLLAMA_CONTEXT_BUCKETS:
        if needed_tokens <= bucket:
            return bucket
    return OLLAMA_CONTEXT_BUCKETS[-1]


def translate_summary_text(summary_text, target_language, model="mistral:latest", prompt_template=None):
    if target_language not in LANG_MAP:
        raise ValueError("Supported languages: de (German), jp (Japanese)")
    prompt = (
        render_translation_prompt(summary_text, target_language, prompt_template)
    )
    payload = {
        "model": model,
        "messages": [
            {"role": "system", "content": f"You are an expert translator proficient in {LANG_MAP[target_language]} and English."},
            {"role": "user", "content": prompt}
        ],
        "options": {
            "num_ctx": choose_ollama_num_ctx(prompt)
        },
        "stream": False
    }
    resp = requests.post("http://localhost:11434/api/chat", json=payload)
    resp.raise_for_status()
    data = resp.json()
    return data.get("message", {}).get("content", "").strip()


def translate_summary_file(summary_file, target_language, model="mistral:latest", prompt_template=None):
    with open(summary_file, "r", encoding="utf-8") as f:
        summary_text = f.read().strip()
    if not summary_text:
        raise ValueError("Empty summary text!")
    return translate_summary_text(summary_text, target_language, model, prompt_template)

def main():
    parser = argparse.ArgumentParser(description="Translate summary using Ollama")
    parser.add_argument("--summary-file", required=True, help="Path to file with English summary text")
    parser.add_argument("--lang", required=True, choices=["de", "jp"], help="Target language: 'de' or 'jp'")
    parser.add_argument("--model", default="mistral:latest", help="Ollama model to use")
    parser.add_argument("--prompt-template", help="Prompt template for the translation LLM call")
    parser.add_argument("--prompt-template-file", help="Path to a text file containing the translation prompt template")
    parser.add_argument("--output-file", help="Output file for translated summary")
    args = parser.parse_args()

    prompt_template = args.prompt_template
    if args.prompt_template_file:
        with open(args.prompt_template_file, "r", encoding="utf-8") as f:
            prompt_template = f.read()

    # Read summary
    try:
        translation = translate_summary_file(args.summary_file, args.lang, args.model, prompt_template)
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
