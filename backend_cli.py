#!/usr/bin/env python3
"""
Single CLI entrypoint for the bundled summarizer backend.

This wrapper lets the Tauri app launch one helper executable in production
while still supporting direct Python execution during development.
"""

import argparse
import json
import sys
from pathlib import Path

from translate_summary import translate_summary_text
from youtube_summarizer import process_video


DEFAULT_MODEL = "mistral:latest"


def configure_stdio() -> None:
    """Keep progress output line-buffered for the desktop app."""
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(line_buffering=True)
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(line_buffering=True)


def summarize(args: argparse.Namespace) -> int:
    meta = process_video(
        args.url,
        use_whisper=args.use_whisper,
        model=args.model,
        output_json=args.output_json,
    )
    if not args.output_json:
        print(json.dumps(meta, ensure_ascii=False), flush=True)
    return 0


def translate(args: argparse.Namespace) -> int:
    summary_path = Path(args.summary_file)
    summary_text = summary_path.read_text(encoding="utf-8").strip()
    if not summary_text:
        raise SystemExit("Empty summary text!")

    translation = translate_summary_text(summary_text, args.lang, args.model)

    if args.output_file:
        Path(args.output_file).write_text(translation, encoding="utf-8")
    else:
        print(translation, flush=True)
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Bundled backend for YouTube Summarizer")
    subparsers = parser.add_subparsers(dest="command", required=True)

    summarize_parser = subparsers.add_parser("summarize", help="Summarize a YouTube video")
    summarize_parser.add_argument("--url", required=True, help="YouTube video URL")
    summarize_parser.add_argument("--model", default=DEFAULT_MODEL, help="Ollama model to use")
    summarize_parser.add_argument(
        "--no-whisper",
        dest="use_whisper",
        action="store_false",
        help="Use transcript/subtitle workflows instead of Whisper",
    )
    summarize_parser.add_argument(
        "--output-json",
        help="Write the result metadata to a JSON file instead of stdout",
    )
    summarize_parser.set_defaults(use_whisper=True, handler=summarize)

    translate_parser = subparsers.add_parser("translate", help="Translate an English summary")
    translate_parser.add_argument("--summary-file", required=True, help="Path to the English summary text")
    translate_parser.add_argument("--lang", required=True, choices=["de", "jp"], help="Target language")
    translate_parser.add_argument("--model", default=DEFAULT_MODEL, help="Ollama model to use")
    translate_parser.add_argument("--output-file", help="Optional path to write the translated text")
    translate_parser.set_defaults(handler=translate)

    return parser


def main() -> int:
    configure_stdio()
    parser = build_parser()
    args = parser.parse_args()
    return args.handler(args)


if __name__ == "__main__":
    raise SystemExit(main())
