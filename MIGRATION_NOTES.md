# Migration Notes

## What Was Preserved

- Static frontend design from `ui/index.html`, including the rose color palette, compact header form, list layout, collapsed summary previews and pagination.
- Frontend behavior from `ui/renderer.js`: model loading, local UI preferences, Whisper toggle, auto-translation toggle, per-entry language tabs, delete confirmation, progress updates, expandable summaries and thumbnail external links.
- Tauri command surface: `get_models`, `get_summaries`, `summarize_video`, `delete_summary`, `translate_summary`, `open_external` and `open_file`.
- Local runtime model: SQLite history in the OS app local data directory, media under that data directory, Ollama on `localhost:11434`, and Python helpers for YouTube metadata, transcripts, Whisper, summaries and translation.
- Release bundling path: a PyInstaller-built backend sidecar plus copied `ffmpeg` and `ffprobe` resources under `src-tauri/resources`.

## Electron Reality Check

No active Electron app was present in the source snapshot used for this migration. There was no Electron main process, preload script, `ipcMain`/`ipcRenderer` bridge, `BrowserWindow` setup, `package.json` or Electron build configuration. The working desktop shell was already Tauri 2, so this folder packages that actual implementation as a standalone Tauri project rather than inventing behavior from missing Electron files.

## Important Runtime Details

- The Tauri identifier remains `com.victorgiers.youtube-summarizer` so OS-level app data and history stay aligned with the existing app identity.
- `run.sh` and `run.bat` now change into this folder before creating the Python environment or launching Cargo.
- The frontend still uses `window.__TAURI__` because `withGlobalTauri` is enabled in `src-tauri/tauri.conf.json`.
- Development falls back to local Python scripts when no bundled backend sidecar exists.

## Imported Legacy Data

- The old Electron database from `/Users/giers/Tools/victors-tools/youtube_summarizer/summaries.db` was copied into the Tauri runtime data directory at `/Users/giers/Library/Application Support/com.victorgiers.youtube-summarizer/summaries.db`.
- A copy also exists at `summaries.db` in this folder for local migration reference.
- Thumbnail files from the old `data/` folder were copied so historical entries keep their images. Audio and transcript files were not copied because the Tauri runtime clears those artifact references on startup.
