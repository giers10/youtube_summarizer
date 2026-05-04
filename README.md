# YouTube Summarizer Tauri

This is a local-first desktop app for summarizing YouTube videos with Ollama.

It uses:

- Tauri for the desktop shell
- a bundled Python backend for transcript/audio processing in release builds
- Ollama on `localhost` for summarization and translation
- SQLite for local history

## What It Does

Given a YouTube URL, the app can:

- fetch a transcript via the YouTube transcript API or via Whisper
- generate an English summary with a local Ollama model
- optionally translate that summary into German and Japanese
- store the results locally so they can be reopened later
- optionally send individual summaries to Discord with a user-provided webhook URL


## Developer Requirements

For development in this repo you still need:

- Python 3.8+
- Rust/Cargo
- FFmpeg in `PATH`
- Ollama running locally on `http://localhost:11434`

Python dependencies are listed in [requirements.txt](requirements.txt).

## Run In Development

macOS/Linux:

```bash
./run.sh
```

Windows:

```bat
run.bat
```

Or directly:

```bash
python3 -m venv venv
source venv/bin/activate
pip install -r requirements.txt
cargo run --manifest-path src-tauri/Cargo.toml
```

The app prefers a bundled backend executable when one is present under [src-tauri/resources/backend](src-tauri/resources/backend), and otherwise falls back to the local Python environment for development.

## Build A Shippable Bundle

1. Make sure the build machine has Python, Rust/Cargo, and `ffmpeg` / `ffprobe` available on `PATH`.
2. Run:

```bash
python3 tools/prepare_bundle.py
```

3. Then build the installer:

```bash
cargo tauri build
```

What `tools/prepare_bundle.py` does:

- installs PyInstaller into the current Python environment
- builds a single-file backend executable from [backend_cli.py](backend_cli.py)
- copies that executable into [src-tauri/resources/backend](src-tauri/resources/backend)
- copies `ffmpeg` and `ffprobe` from the build machine into [src-tauri/resources/ffmpeg](src-tauri/resources/ffmpeg)

Build once on each target OS you want to ship. For Windows 10, build on Windows.

## Notes

- If Python is not on your `PATH` for development, set `YTS_PYTHON` to the interpreter you want the Tauri backend to use.
- If you want to test a prebuilt backend executable during development, set `YTS_BACKEND_BIN` to its full path.
- If `ffmpeg` or `ffprobe` are not on `PATH` during bundle prep, set `YTS_FFMPEG` and `YTS_FFPROBE` to their full paths before running [tools/prepare_bundle.py](tools/prepare_bundle.py).
- Generated thumbnails and the SQLite database are created on first run in the app's local data directory.
