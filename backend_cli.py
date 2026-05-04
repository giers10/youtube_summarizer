#!/usr/bin/env python3
"""
Single CLI entrypoint for the bundled summarizer backend.

This wrapper lets the Tauri app launch one helper executable in production
while still supporting direct Python execution during development.
"""

import argparse
import json
import multiprocessing
import sys
from pathlib import Path

from translate_summary import translate_summary_text
from youtube_summarizer import process_video


DEFAULT_MODEL = "mistral:latest"


def compact_error_message(exc: BaseException) -> str:
    """Build a short error string without dumping a traceback into the GUI."""
    parts = []
    current = exc
    while current:
        text = " ".join(str(current).split())
        if text and text not in parts:
            parts.append(text)
        current = current.__cause__ or current.__context__
    return ": ".join(parts) or exc.__class__.__name__


def configure_stdio() -> None:
    """Keep progress output line-buffered for the desktop app."""
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace", line_buffering=True)
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8", errors="replace", line_buffering=True)


def summarize(args: argparse.Namespace) -> int:
    prompt_template = None
    if args.prompt_template_file:
        prompt_template = Path(args.prompt_template_file).read_text(encoding="utf-8")
    elif args.prompt_template:
        prompt_template = args.prompt_template

    meta = process_video(
        args.url,
        use_whisper=args.use_whisper,
        model=args.model,
        prompt_template=prompt_template,
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

    prompt_template = None
    if args.prompt_template_file:
        prompt_template = Path(args.prompt_template_file).read_text(encoding="utf-8")
    elif args.prompt_template:
        prompt_template = args.prompt_template

    translation = translate_summary_text(summary_text, args.lang, args.model, prompt_template)

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
    summarize_parser.add_argument("--prompt-template", help="Prompt template for the summary LLM call")
    summarize_parser.add_argument("--prompt-template-file", help="Path to a prompt template file")
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
    translate_parser.add_argument("--prompt-template", help="Prompt template for the translation LLM call")
    translate_parser.add_argument("--prompt-template-file", help="Path to a translation prompt template file")
    translate_parser.add_argument("--output-file", help="Optional path to write the translated text")
    translate_parser.set_defaults(handler=translate)

    return parser


def main() -> int:
    multiprocessing.freeze_support()
    configure_stdio()
    parser = build_parser()
    args = parser.parse_args()
    try:
        return args.handler(args)
    except KeyboardInterrupt:
        print("[error] Cancelled.", file=sys.stderr, flush=True)
        return 130
    except Exception as exc:
        print(f"[error] {compact_error_message(exc)}", file=sys.stderr, flush=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
