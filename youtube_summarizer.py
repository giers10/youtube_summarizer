#!/usr/bin/env python3
"""
youtube_summarizer.py

This script accepts a YouTube URL, retrieves a transcript either via the
YouTube API or via Whisper (depending on the flags), generates a concise
summary using Ollama and optionally writes a JSON descriptor containing
metadata about the processed video.  The metadata includes the video
identifier, original URL, title, downloaded thumbnail filename, audio
filename, transcript filename and the summary text itself.  The script
has been adapted from an earlier command‑line tool to better integrate
with a GUI.  The summarizer now returns the summary text instead of
printing it directly and supports additional command line arguments for
JSON output.

Usage:
    python3 youtube_summarizer.py <youtube-url> [--no-ai] [--output-json <path>]

Options:
    --no-ai       Use the classic API/subtitle workflow instead of Whisper for
                  transcription (default uses Whisper).
    --output-json Specify a file path where metadata about the processed video
                  will be written as JSON.  If omitted the metadata is
                  printed to standard output in JSON format.

This script relies on yt_dlp for fetching video metadata, requests for
thumbnail download and the whisper and youtube_transcript_api packages for
transcription.

"""

import sys
import os
import re
import time
import json
import glob
import math
import subprocess
import multiprocessing
import threading
import requests
import yt_dlp
import webvtt
from datetime import datetime
from typing import List, Tuple, Optional
from xml.parsers.expat import ExpatError
from xml.etree.ElementTree import ParseError
from youtube_transcript_api import YouTubeTranscriptApi
from youtube_transcript_api._errors import TranscriptsDisabled, NoTranscriptFound

try:
    import whisper
except ImportError:
    whisper = None  # handle gracefully if whisper isn't installed

# -----------------------
# Konfiguration & Flags
# -----------------------
DEBUG = False

# Whisper‑Settings
NUM_SLICES = 8
OVERLAP_SEC = 1
MAX_OVERLAP_WORDS = 7
WHISPER_MODEL = "small"  # e.g. "small", "medium", "large-v3" …
OLLAMA_CHARS_PER_TOKEN = 3.5
OLLAMA_OUTPUT_TOKEN_BUDGET = 2048
OLLAMA_CONTEXT_BUCKETS = (4096, 8192, 16384, 32768, 65536)
DEFAULT_SUMMARY_PROMPT_TEMPLATE = """You are an expert summarizer. Summarize the following video concisely:

Title: {title}

Transcript:
{transcript}

Summary:"""


def debug_print(*args, **kwargs):
    """Print debug messages when DEBUG is enabled."""
    if DEBUG:
        print("[DEBUG]", *args, **kwargs, file=sys.stderr)


class ProgressHeartbeat:
    """Emit periodic progress while a blocking backend operation is active."""

    def __init__(self, message_fn, interval: float = 15.0):
        self.message_fn = message_fn
        self.interval = interval
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def __enter__(self):
        self._thread.start()
        return self

    def __exit__(self, exc_type, exc, tb):
        self._stop.set()
        self._thread.join(timeout=1.0)

    def _run(self):
        while not self._stop.wait(self.interval):
            message = self.message_fn()
            if message:
                print(message, flush=True)


def get_ffmpeg_binary() -> str:
    """Return the ffmpeg executable path, preferring a bundled override."""
    value = os.environ.get("YTS_FFMPEG", "").strip()
    return value or "ffmpeg"


def get_ffprobe_binary() -> str:
    """Return the ffprobe executable path, preferring a bundled override."""
    value = os.environ.get("YTS_FFPROBE", "").strip()
    return value or "ffprobe"


def get_ffmpeg_directory() -> Optional[str]:
    """Return the directory containing the configured ffmpeg binary."""
    value = os.environ.get("YTS_FFMPEG", "").strip()
    if not value:
        return None
    if os.path.isfile(value):
        return os.path.dirname(value)
    return value


def get_yt_dlp_ffmpeg_location() -> Optional[str]:
    """Return an ffmpeg location suitable for yt_dlp postprocessors."""
    return get_ffmpeg_directory()


def ensure_ffmpeg_on_path() -> None:
    """Expose bundled ffmpeg to libraries that shell out to plain `ffmpeg`."""
    ffmpeg_dir = get_ffmpeg_directory()
    if not ffmpeg_dir:
        return
    path_entries = [entry for entry in os.environ.get("PATH", "").split(os.pathsep) if entry]
    if ffmpeg_dir not in path_entries:
        os.environ["PATH"] = os.pathsep.join([ffmpeg_dir, *path_entries])


def get_whisper_download_root() -> Optional[str]:
    """Return a stable Whisper cache directory when one is configured."""
    value = os.environ.get("YTS_WHISPER_CACHE_DIR", "").strip()
    if not value:
        return None
    os.makedirs(value, exist_ok=True)
    return value


# -----------------------
# 1) Utilities
# -----------------------

def extract_video_id(url: str) -> Optional[str]:
    """Extract the eleven character YouTube video ID from a URL."""
    debug_print(f"Extracting video ID from URL: {url}")
    m = re.search(r'(?:v=|youtu\.be/)([0-9A-Za-z_-]{11})', url)
    vid = m.group(1) if m else None
    debug_print(f"Video ID: {vid}")
    return vid


def get_transcript_api(video_id: str) -> str:
    """
    Fetch transcript via YouTubeTranscriptApi, trying 'en', then 'de', then any available language.
    """
    debug_print(f"Trying transcript API for {video_id}")

    # Try English first
    try:
        data = YouTubeTranscriptApi.get_transcript(video_id, languages=["en"])
        text = " ".join(item["text"] for item in data)
        debug_print(f"Transcript fetched in EN, length {len(text)} chars")
        return text
    except (TranscriptsDisabled, NoTranscriptFound):
        pass

    # Try German
    try:
        data = YouTubeTranscriptApi.get_transcript(video_id, languages=["de"])
        text = " ".join(item["text"] for item in data)
        debug_print(f"Transcript fetched in DE, length {len(text)} chars")
        return text
    except (TranscriptsDisabled, NoTranscriptFound):
        pass

    # Try any available language (prefer auto-generated if possible)
    try:
        tx_list = YouTubeTranscriptApi.list_transcripts(video_id)
        # Try manually created first
        for tr in tx_list:
            try:
                if not tr.is_generated:
                    data = tr.fetch()
                    text = " ".join(item["text"] for item in data)
                    debug_print(f"Transcript fetched: {tr.language_code} (manual)")
                    return text
            except Exception:
                continue
        # Then fallback to auto-generated
        for tr in tx_list:
            try:
                if tr.is_generated:
                    data = tr.fetch()
                    text = " ".join(item["text"] for item in data)
                    debug_print(f"Transcript fetched: {tr.language_code} (auto-generated)")
                    return text
            except Exception:
                continue
    except Exception as e:
        debug_print(f"list_transcripts failed: {e}")

    # Nothing found, fail with info
    raise SystemExit(
        "No transcript available in EN, DE or any other language via API. "
        "Try 'Use Whisper' mode or wait if you hit a YouTube rate limit."
    )


def vtt_to_lines(path: str) -> List[str]:
    """Convert a VTT file into deduplicated lines of text."""
    cues, last = [], None
    for caption in webvtt.read(path):
        cur = caption.text.replace("\n", " ").strip()
        if not cur or cur == last:
            continue
        if last and cur.startswith(last):
            cur = cur[len(last):].strip(" -")
        cues.append(cur)
        last = caption.text.replace("\n", " ").strip()
    return cues


def remove_consecutive_line_duplicates(lines: List[str]) -> List[str]:
    """Remove consecutive duplicate lines."""
    deduped, last = [], None
    for l in lines:
        if l != last:
            deduped.append(l)
        last = l
    return deduped


def remove_phrase_duplicates_from_lines(lines: List[str]) -> List[str]:
    """Remove duplicate phrases within lines (used for subtitle deduplication)."""
    out, last = [], None
    for l in lines:
        if last and l.startswith(last):
            trimmed = l[len(last):].strip()
            if trimmed:
                out.append(trimmed)
        else:
            out.append(l)
        last = l
    return out


def remove_empty_lines(lines: List[str]) -> List[str]:
    """Remove empty lines."""
    return [l for l in lines if l.strip()]


YOUTUBE_AUTH_ERROR_MARKERS = (
    "sign in to confirm",
    "not a bot",
    "login_required",
    "use --cookies-from-browser",
    "use --cookies for the authentication",
)

YOUTUBE_JS_CHALLENGE_ERROR_MARKERS = (
    "n challenge solving failed",
    "only images are available",
    "requested format is not available",
    "remote component challenge solver",
    "sabr streaming",
)


def parse_cookies_from_browser_spec(
    spec: Optional[str],
) -> Optional[Tuple[Optional[str], Optional[str], Optional[str], Optional[str]]]:
    """Parse yt-dlp's browser cookie spec into the tuple YoutubeDL expects."""
    if not spec:
        return None

    raw = spec.strip()
    if not raw:
        return None

    container = None
    browser_part = raw
    if "::" in browser_part:
        browser_part, container = browser_part.split("::", 1)
        container = container or None

    profile = None
    if ":" in browser_part:
        browser_part, profile = browser_part.split(":", 1)
        profile = profile or None

    keyring = None
    if "+" in browser_part:
        browser, keyring = browser_part.split("+", 1)
        keyring = keyring or None
    else:
        browser = browser_part

    browser = browser.strip().lower()
    if not browser:
        raise ValueError("Browser cookie source is missing a browser name.")
    return (browser, profile, keyring, container)


def apply_ytdlp_cookie_options(
    opts: dict,
    cookies_from_browser: Optional[str] = None,
    cookies_file: Optional[str] = None,
    use_js_challenge_fallback: bool = False,
) -> dict:
    """Attach cookie and challenge-solver options to a yt-dlp option dictionary."""
    if cookies_file:
        opts["cookiefile"] = cookies_file
    browser_spec = parse_cookies_from_browser_spec(cookies_from_browser)
    if browser_spec:
        opts["cookiesfrombrowser"] = browser_spec
    if use_js_challenge_fallback:
        opts["js_runtimes"] = ["deno", "node"]
        opts["remote_components"] = ["ejs:github"]
    return opts


def exception_chain_text(exc: BaseException) -> str:
    parts = []
    current = exc
    while current:
        parts.append(str(current))
        current = current.__cause__ or current.__context__
    return "\n".join(parts).lower()


def is_youtube_auth_error(exc: BaseException) -> bool:
    text = exception_chain_text(exc)
    return any(marker in text for marker in YOUTUBE_AUTH_ERROR_MARKERS)


def is_youtube_js_challenge_error(exc: BaseException) -> bool:
    text = exception_chain_text(exc)
    return any(marker in text for marker in YOUTUBE_JS_CHALLENGE_ERROR_MARKERS)


def has_cookie_source(cookies_from_browser: Optional[str], cookies_file: Optional[str]) -> bool:
    return bool((cookies_from_browser or "").strip() or (cookies_file or "").strip())


def get_subtitles_via_yt_dlp(
    url: str,
    cookies_from_browser: Optional[str] = None,
    cookies_file: Optional[str] = None,
    use_js_challenge_fallback: bool = False,
) -> Optional[str]:
    """Try to fetch subtitles via yt_dlp when API transcripts fail."""
    debug_print(f"Fetching metadata via yt‑dlp for URL: {url}")
    opts = apply_ytdlp_cookie_options(
        {'skip_download': True, 'quiet': True, 'ignoreerrors': True},
        cookies_from_browser,
        cookies_file,
        use_js_challenge_fallback,
    )
    with yt_dlp.YoutubeDL(opts) as ydl:
        info = ydl.extract_info(url, download=False)
    available = list(info.get('subtitles', {})) + list(info.get('automatic_captions', {}))
    debug_print(f"Available subtitle languages: {available}")
    if not available:
        return None

    priority = ['en', 'es', 'fr', 'de', 'zh', 'ja']
    langs = [l for l in priority if l in available] + [l for l in available if l not in priority]

    for lang in langs:
        debug_print(f"Trying subtitle language {lang}")
        dl_opts = apply_ytdlp_cookie_options({
            'skip_download': True,
            'writesubtitles': True,
            'writeautomaticsub': True,
            'subtitlesformat': 'vtt',
            'subtitlelangs': [lang],
            'outtmpl': "transcript.%(language)s.%(ext)s",
            'quiet': True,
        }, cookies_from_browser, cookies_file, use_js_challenge_fallback)
        with yt_dlp.YoutubeDL(dl_opts) as ydl:
            ydl.download([url])

        files = [f for f in os.listdir('.') if f.startswith('transcript') and f.endswith('.vtt')]
        if not files:
            continue
        path = files[0]
        try:
            lines = vtt_to_lines(path)
            lines = remove_consecutive_line_duplicates(lines)
            lines = remove_phrase_duplicates_from_lines(lines)
            lines = remove_empty_lines(lines)
            text = "\n".join(lines)
            debug_print(f"Subtitle text length: {len(text)}")
            return text
        except Exception as e:
            debug_print(f"Subtitle parsing failed: {e}")
    return None


# --------------------------
# 2) Whisper‑based workflow
# --------------------------

def _cleanup_audio_artifacts(vid: str) -> None:
    """Remove partial audio download artifacts for the given video id."""
    for path in glob.glob(f"audio_{vid}.*"):
        # Keep any existing mp3; it may belong to a previous summary.
        if path.endswith(".mp3"):
            continue
        try:
            os.remove(path)
        except OSError:
            pass


def _download_audio_with_yt_dlp(
    url: str,
    vid: str,
    extractor_args: Optional[dict] = None,
    cookies_from_browser: Optional[str] = None,
    cookies_file: Optional[str] = None,
    use_js_challenge_fallback: bool = False,
) -> str:
    """Download audio via yt_dlp and extract to wav."""
    audio_fn = f"audio_{vid}.wav"
    opts = apply_ytdlp_cookie_options({
        "format": "bestaudio/best",
        "outtmpl": f"audio_{vid}.%(ext)s",
        "quiet": True,
        "noprogress": True,
        "nopart": True,
        "continuedl": False,
        "overwrites": True,
        "noplaylist": True,
        "retries": 3,
        "fragment_retries": 3,
        "postprocessors": [{
            "key": "FFmpegExtractAudio",
            "preferredcodec": "wav",
        }],
    }, cookies_from_browser, cookies_file, use_js_challenge_fallback)
    if extractor_args:
        opts["extractor_args"] = extractor_args
    ffmpeg_location = get_yt_dlp_ffmpeg_location()
    if ffmpeg_location:
        opts["ffmpeg_location"] = ffmpeg_location
    with yt_dlp.YoutubeDL(opts) as ydl:
        ydl.download([url])
    if not os.path.exists(audio_fn):
        raise RuntimeError("yt_dlp completed but wav file was not created")
    return audio_fn


def download_video_audio(
    url: str,
    vid: str,
    cookies_from_browser: Optional[str] = None,
    cookies_file: Optional[str] = None,
    use_js_challenge_fallback: bool = False,
) -> str:
    """Download the best available audio for a YouTube video."""
    ensure_ffmpeg_on_path()
    print(f"Downloading audio from {url} ...")

    # Clean up any stale partials that can trigger HTTP 416 resume errors.
    _cleanup_audio_artifacts(vid)

    attempts = [
        ("android player client", {"youtube": {"player_client": ["android"]}}),
        ("default player client", None),
    ]

    last_err = None
    for label, extractor_args in attempts:
        try:
            debug_print(f"yt_dlp audio attempt: {label}")
            audio_fn = _download_audio_with_yt_dlp(
                url,
                vid,
                extractor_args,
                cookies_from_browser,
                cookies_file,
                use_js_challenge_fallback,
            )
            debug_print(f"Audio saved as {audio_fn}")
            return audio_fn
        except Exception as e:
            last_err = e
            debug_print(f"yt_dlp attempt failed ({label}): {e}")
            _cleanup_audio_artifacts(vid)

    raise RuntimeError("Audio download failed after multiple attempts") from last_err


def get_audio_duration(path: str) -> float:
    """Return the duration of an audio file using ffprobe."""
    try:
        file_size = os.path.getsize(path)
    except OSError:
        file_size = None

    ffprobe_args = [
        get_ffprobe_binary(), "-v", "error", "-show_entries", "format=duration",
        "-of", "default=noprint_wrappers=1:nokey=1", path
    ]
    res = subprocess.run(
        ffprobe_args,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )

    print(f"[ffprobe] command: {ffprobe_args}", file=sys.stderr, flush=True)
    print(f"[ffprobe] target: {path}", file=sys.stderr, flush=True)
    print(f"[ffprobe] target size bytes: {file_size}", file=sys.stderr, flush=True)
    print(f"[ffprobe] returncode: {res.returncode}", file=sys.stderr, flush=True)
    print("[ffprobe] stdout raw begin", file=sys.stderr, flush=True)
    if res.stdout:
        sys.stderr.write(res.stdout)
        if not res.stdout.endswith("\n"):
            sys.stderr.write("\n")
        sys.stderr.flush()
    else:
        print("<empty>", file=sys.stderr, flush=True)
    print("[ffprobe] stdout raw end", file=sys.stderr, flush=True)
    print("[ffprobe] stderr raw begin", file=sys.stderr, flush=True)
    if res.stderr:
        sys.stderr.write(res.stderr)
        if not res.stderr.endswith("\n"):
            sys.stderr.write("\n")
        sys.stderr.flush()
    else:
        print("<empty>", file=sys.stderr, flush=True)
    print("[ffprobe] stderr raw end", file=sys.stderr, flush=True)

    stdout_value = res.stdout.strip()
    try:
        return float(stdout_value)
    except ValueError:
        match = re.search(r"[-+]?\d+(?:[.,]\d+)?", stdout_value or res.stderr)
        if match:
            return float(match.group(0).replace(",", "."))

    raise RuntimeError(
        "ffprobe did not return a parseable duration "
        f"(returncode={res.returncode}, stdout={stdout_value!r}, stderr={res.stderr.strip()!r})"
    )


def slice_audio(audio_path: str, vid: str) -> List[Tuple[str, float, float]]:
    """Slice a long audio file into overlapping chunks for Whisper."""
    print("Slicing audio ...")
    duration = get_audio_duration(audio_path)
    length = duration / NUM_SLICES
    slices = []
    for i in range(NUM_SLICES):
        start = max(0, i * length - (OVERLAP_SEC if i > 0 else 0))
        end = min(duration, (i + 1) * length + (OVERLAP_SEC if i < NUM_SLICES - 1 else 0))
        fn = f"audio_{vid}_slice_{i:02d}.wav"
        subprocess.run([
            get_ffmpeg_binary(), "-y", "-hide_banner", "-loglevel", "error",
            "-ss", str(start), "-to", str(end),
            "-i", audio_path, "-acodec", "copy", fn
        ], check=True)
        debug_print(f"  slice {i}: {start:.1f}s→{end:.1f}s ({fn})")
        slices.append((fn, start, end))
    return slices


def transcribe_slice(args: Tuple[str, int, str, str]) -> str:
    """Transcribe a single audio slice using Whisper and save to a text file."""
    ensure_ffmpeg_on_path()
    slice_path, idx, model_name, vid = args
    if whisper is None:
        raise RuntimeError("Whisper package is required but not installed")
    m = whisper.load_model(model_name, download_root=get_whisper_download_root())
    res = m.transcribe(slice_path, task="transcribe")
    out = f"transcript_{vid}_slice_{idx:02d}.txt"
    with open(out, "w", encoding="utf-8") as f:
        f.write(res["text"])
    debug_print(f"Transcribed slice {idx} → {out}")
    return out


def merge_transcripts(files: List[str]) -> str:
    """Merge transcribed slices by eliminating overlapping words."""
    merged, prev = [], []
    for i, fn in enumerate(files):
        words = open(fn, encoding="utf-8").read().split()
        if i > 0:
            p_tail = prev[-MAX_OVERLAP_WORDS:]
            c_head = words[:MAX_OVERLAP_WORDS]
            L = min(len(p_tail), len(c_head))
            best = 0
            for n in range(L, 4, -1):
                if p_tail[-n:] == c_head[:n]:
                    best = n
                    break
            if best:
                debug_print(f"  overlap {best} words between slices {i-1}↔{i}")
                words = words[best:]
        merged += words
        prev = words
    text = " ".join(merged)
    debug_print(f"Merged transcript: {len(text)} chars, {len(merged)} words")
    return text


def clean_temp(pattern: str) -> None:
    """Remove temporary files matching the given glob pattern."""
    for f in glob.glob(pattern):
        try:
            os.remove(f)
        except Exception:
            pass


def whisper_transcript(
    url: str,
    vid: str,
    cookies_from_browser: Optional[str] = None,
    cookies_file: Optional[str] = None,
    use_js_challenge_fallback: bool = False,
) -> str:
    """Run the Whisper pipeline and return the final transcript text."""
    ensure_ffmpeg_on_path()
    audio = download_video_audio(
        url,
        vid,
        cookies_from_browser,
        cookies_file,
        use_js_challenge_fallback,
    )
    slices = slice_audio(audio, vid)
    print("Transcribing using Whisper...", flush=True)
    args = [(p, i, WHISPER_MODEL, vid) for i, (p, _, _) in enumerate(slices)]
    with multiprocessing.Pool(len(slices)) as pool:
        t_files = pool.map(transcribe_slice, args)
    text = merge_transcripts(t_files)
    clean_temp(f"audio_{vid}_slice_*.wav")
    clean_temp(f"transcript_{vid}_slice_*.txt")
    # Leave the original audio file so it can be referenced by the GUI
    return text


# -----------------------
# Ollama‑Summarizer
# -----------------------

def render_summary_prompt(title: str, transcript: str, prompt_template: Optional[str] = None) -> str:
    template = (prompt_template or DEFAULT_SUMMARY_PROMPT_TEMPLATE).strip()
    prompt = template.replace("{title}", title).replace("{transcript}", transcript)
    if "{title}" not in template:
        prompt = f"{prompt}\n\nTitle: {title}"
    if "{transcript}" not in template:
        prompt = f"{prompt}\n\nTranscript:\n{transcript}"
    return prompt


def choose_ollama_num_ctx(prompt: str, output_budget: int = OLLAMA_OUTPUT_TOKEN_BUDGET) -> int:
    estimated_input_tokens = math.ceil(len(prompt) / OLLAMA_CHARS_PER_TOKEN)
    needed_tokens = estimated_input_tokens + output_budget
    for bucket in OLLAMA_CONTEXT_BUCKETS:
        if needed_tokens <= bucket:
            return bucket
    return OLLAMA_CONTEXT_BUCKETS[-1]


def summarize_with_ollama(
    title: str,
    transcript: str,
    model: str = "mistral:latest",
    prompt_template: Optional[str] = None,
) -> str:
    """
    Send video title and transcript text to Ollama and return the summary string.
    """
    debug_print(f"Preparing summary with model {model}, transcript length={len(transcript)}")
    prompt = render_summary_prompt(title, transcript, prompt_template)
    debug_print(prompt)
    payload = {
        "model": model,
        "messages": [
            {"role": "system", "content": "You are an intelligent summarizer."},
            {"role": "user", "content": prompt}
        ],
        "options": {
            "num_ctx": choose_ollama_num_ctx(prompt)
        },
        "stream": True
    }
    debug_print("Sending request to Ollama ...")
    summary = ""
    last_progress_chars = 0

    def heartbeat_message() -> str:
        if summary:
            return f"Ollama is generating summary... {len(summary)} characters received."
        return "Waiting for Ollama to start responding..."

    try:
        with ProgressHeartbeat(heartbeat_message):
            resp = requests.post(
                "http://localhost:11434/api/chat",
                json=payload,
                stream=True,
                timeout=(10, 1800),
            )
            debug_print(f"Ollama status: {resp.status_code}")
            resp.raise_for_status()
            for line in resp.iter_lines(decode_unicode=True):
                if not line:
                    continue
                try:
                    msg = json.loads(line).get("message", {}).get("content", "")
                    summary += msg
                    if len(summary) - last_progress_chars >= 1000:
                        last_progress_chars = len(summary)
                        print(
                            f"Ollama is generating summary... {last_progress_chars} characters received.",
                            flush=True,
                        )
                except Exception:
                    continue
    except requests.RequestException as exc:
        raise RuntimeError(f"Ollama request failed: {exc}") from exc
    if not summary.strip():
        raise RuntimeError("Ollama returned an empty summary.")
    debug_print(f"Summary generated, length={len(summary)}")
    print("Summary generated.", flush=True)
    return summary


# -----------------------
# Video metadata and thumbnail download
# -----------------------

def fetch_video_metadata(
    url: str,
    cookies_from_browser: Optional[str] = None,
    cookies_file: Optional[str] = None,
) -> Tuple[str, str, str]:
    """
    Fetch the title, thumbnail URL and video ID for a YouTube URL using yt_dlp.
    Returns a tuple: (video_id, title, thumbnail_url)
    """
    opts = apply_ytdlp_cookie_options({'quiet': True}, cookies_from_browser, cookies_file)
    with yt_dlp.YoutubeDL(opts) as ydl:
        info = ydl.extract_info(url, download=False)
    vid = info.get('id')
    title = info.get('title', f"Video {vid}")
    thumbnail_url = info.get('thumbnail')
    return vid, title, thumbnail_url


def fetch_channel_name(
    url: str,
    cookies_from_browser: Optional[str] = None,
    cookies_file: Optional[str] = None,
) -> Optional[str]:
    """
    Retrieve the channel or uploader name for a YouTube video using yt_dlp.
    Returns None if it cannot be determined.
    """
    try:
        opts = apply_ytdlp_cookie_options({'quiet': True}, cookies_from_browser, cookies_file)
        with yt_dlp.YoutubeDL(opts) as ydl:
            info = ydl.extract_info(url, download=False)
        # Try channel, uploader, then return None
        return info.get('channel') or info.get('uploader')
    except Exception as e:
        debug_print(f"Failed to fetch channel name: {e}")
        return None


def download_thumbnail(vid: str, thumbnail_url: str) -> Optional[str]:
    """
    Download the thumbnail image given its URL and save it as thumb_<vid>.<ext>.
    Returns the local filename or None if download fails.
    """
    if not thumbnail_url:
        return None
    try:
        response = requests.get(thumbnail_url, timeout=10)
        response.raise_for_status()
        # Determine extension from content type or URL
        ext = None
        if 'content-type' in response.headers:
            ctype = response.headers['content-type']
            if 'jpeg' in ctype:
                ext = 'jpg'
            elif 'png' in ctype:
                ext = 'png'
        if ext is None:
            ext = thumbnail_url.split('.')[-1].split('?')[0]
        filename = f"thumb_{vid}.{ext}"
        with open(filename, 'wb') as f:
            f.write(response.content)
        debug_print(f"Thumbnail downloaded as {filename}")
        return filename
    except Exception as e:
        debug_print(f"Thumbnail download failed: {e}")
        return None


# -----------------------
# Main
# -----------------------

def process_video(
    url: str,
    use_whisper: bool,
    model: str = "mistral:latest",
    output_json: Optional[str] = None,
    prompt_template: Optional[str] = None,
    cookies_from_browser: Optional[str] = None,
    cookies_file: Optional[str] = None,
) -> dict:
    try:
        return process_video_once(url, use_whisper, model, output_json, prompt_template)
    except Exception as exc:
        if is_youtube_js_challenge_error(exc):
            print("YouTube player challenge failed; retrying with Node/EJS challenge fallback...", flush=True)
            return process_video_once(
                url,
                use_whisper,
                model,
                output_json,
                prompt_template,
                cookies_from_browser,
                cookies_file,
                True,
            )

        if has_cookie_source(cookies_from_browser, cookies_file) and is_youtube_auth_error(exc):
            print("YouTube requested sign-in; retrying with selected cookies...", flush=True)
            try:
                return process_video_once(
                    url,
                    use_whisper,
                    model,
                    output_json,
                    prompt_template,
                    cookies_from_browser,
                    cookies_file,
                )
            except Exception as cookie_exc:
                if is_youtube_js_challenge_error(cookie_exc):
                    print(
                        "YouTube player challenge failed; retrying with Node/EJS challenge fallback...",
                        flush=True,
                    )
                    return process_video_once(
                        url,
                        use_whisper,
                        model,
                        output_json,
                        prompt_template,
                        cookies_from_browser,
                        cookies_file,
                        True,
                    )
                raise
        raise


def process_video_once(
    url: str,
    use_whisper: bool,
    model: str = "mistral:latest",
    output_json: Optional[str] = None,
    prompt_template: Optional[str] = None,
    cookies_from_browser: Optional[str] = None,
    cookies_file: Optional[str] = None,
    use_js_challenge_fallback: bool = False,
) -> dict:
    """
    Core processing routine.  Retrieves metadata, obtains transcript via the
    selected workflow, generates a summary using Ollama and writes the
    transcript, thumbnail and audio (converted to mp3) to disk.  Returns a
    dictionary containing metadata which may also be dumped to a JSON file if
    output_json is provided.

    Parameters
    ----------
    url : str
        The YouTube video URL.
    use_whisper : bool
        If True, use the Whisper transcription workflow; if False, use the
        classic API/subtitle workflow.
    model : str, optional
        The Ollama model name to use for summarization.  Defaults to
        "mistral:latest".
    output_json : str or None, optional
        If provided, path to a file where JSON metadata should be written.

    Returns
    -------
    dict
        A dictionary containing metadata about the processed video.
    """
    vid, title, thumb_url = fetch_video_metadata(
        url,
        cookies_from_browser,
        cookies_file,
        use_js_challenge_fallback,
    )
    if not vid:
        raise SystemExit("Invalid YouTube URL.")

    # Fetch the channel/uploader name
    channel_name = fetch_channel_name(
        url,
        cookies_from_browser,
        cookies_file,
        use_js_challenge_fallback,
    )

    # Fetch transcript
    if use_whisper:
        print("Using Whisper parallel transcription...")
        transcript_text = whisper_transcript(
            url,
            vid,
            cookies_from_browser,
            cookies_file,
            use_js_challenge_fallback,
        )
        if not transcript_text.strip():
            raise SystemExit("Whisper transcription failed or empty.")
    else:
        print("Using classic API/subtitle workflow...")
        # Try API first
        try:
            transcript_text = get_transcript_api(vid)
        except Exception:
            print("API failed, falling back to subtitles...")
            transcript_text = get_subtitles_via_yt_dlp(
                url,
                cookies_from_browser,
                cookies_file,
                use_js_challenge_fallback,
            )
        if not transcript_text:
            raise SystemExit("No transcript/subtitles available.")

    # Save transcript to file
    transcript_filename = f"transcript_{vid}.txt"
    with open(transcript_filename, 'w', encoding='utf-8') as f:
        f.write(transcript_text)
    debug_print(f"Transcript saved to {transcript_filename}")

    # Download thumbnail
    thumbnail_filename = download_thumbnail(vid, thumb_url)

    # Determine audio filename if generated and convert to mp3
    audio_filename = None
    if use_whisper:
        wav_name = f"audio_{vid}.wav"
        mp3_name = f"audio_{vid}.mp3"
        # Convert to mp3 using ffmpeg if wav exists
        if os.path.exists(wav_name):
            try:
                subprocess.run([
                    get_ffmpeg_binary(), '-y', '-i', wav_name,
                    '-codec:a', 'libmp3lame', '-qscale:a', '2',
                    mp3_name
                ], check=True)
                os.remove(wav_name)
                debug_print(f"Converted {wav_name} to {mp3_name} and removed wav")
                audio_filename = mp3_name
            except Exception as e:
                debug_print(f"Failed to convert audio to mp3: {e}")
                # fallback: keep wav
                audio_filename = wav_name
        else:
            # If wav file doesn't exist yet (perhaps removed elsewhere), do not set audio
            audio_filename = None

    # Generate summary
    print("Generating summary with Ollama...", flush=True)
    summary_text = summarize_with_ollama(title, transcript_text, model, prompt_template)

    # Create metadata dictionary
    meta = {
        'timestamp': datetime.utcnow().isoformat() + 'Z',
        'video_id': vid,
        'url': url,
        'video_name': title,
        'channel': channel_name,
        'thumbnail': thumbnail_filename,
        'audio': audio_filename,
        'transcript': transcript_filename,
        'summary': summary_text
    }

    # Write JSON output if requested
    if output_json:
        with open(output_json, 'w', encoding='utf-8') as f:
            json.dump(meta, f, ensure_ascii=False, indent=2)
        debug_print(f"Metadata written to {output_json}")
    return meta


def rewrite_summary(
    title: str,
    transcript_file: str,
    model: str = "mistral:latest",
    output_json: Optional[str] = None,
    prompt_template: Optional[str] = None,
) -> dict:
    """
    Regenerate a summary from an existing transcript file using the specified model.

    Parameters
    ----------
    transcript_file : str
        Path to a text file containing the transcript.
    model : str, optional
        Name of the Ollama model to use for summarization.
    output_json : str or None, optional
        If provided, write the resulting summary dictionary to this file.

    Returns
    -------
    dict
        A dictionary containing just the summary.
    """
    if not os.path.exists(transcript_file):
        raise SystemExit(f"Transcript file not found: {transcript_file}")
    with open(transcript_file, 'r', encoding='utf-8') as f:
        transcript_text = f.read()
    debug_print(f"Rewriting summary using model {model} for {transcript_file}")
    summary_text = summarize_with_ollama(title, transcript_text, model, prompt_template)
    meta = {'summary': summary_text}
    if output_json:
        with open(output_json, 'w', encoding='utf-8') as f:
            json.dump(meta, f, ensure_ascii=False, indent=2)
        debug_print(f"Summary written to {output_json}")
    return meta


def main():
    import argparse
    parser = argparse.ArgumentParser(description="YouTube → Transcript → Ollama Summary")
    parser.add_argument('url', help="YouTube‑Video‑URL")
    parser.add_argument('--no-ai', action='store_true',
                        help="Use classic API/subtitle workflow instead of Whisper")
    parser.add_argument('--output-json', type=str, default=None,
                        help="Write metadata JSON to the specified file instead of STDOUT")
    parser.add_argument('--model', type=str, default='mistral:latest',
                        help="Ollama model to use for summarization (default: mistral:latest)")
    parser.add_argument('--transcript-file', type=str, default=None,
                        help="Path to an existing transcript file; when provided the script will skip transcription and only generate a summary.")
    parser.add_argument('--prompt-template', type=str, default=None,
                        help="Prompt template for the summary LLM call.")
    parser.add_argument('--prompt-template-file', type=str, default=None,
                        help="Path to a text file containing the prompt template.")
    parser.add_argument('--cookies-from-browser', type=str, default=None,
                        help="Browser cookie source to pass to yt-dlp.")
    parser.add_argument('--cookies-file', type=str, default=None,
                        help="Netscape cookies.txt file to pass to yt-dlp.")
    args = parser.parse_args()

    use_whisper = not args.no_ai
    prompt_template = args.prompt_template
    if args.prompt_template_file:
        with open(args.prompt_template_file, 'r', encoding='utf-8') as f:
            prompt_template = f.read()

    try:
        # If a transcript file is provided, skip the normal processing and only rewrite summary
        if args.transcript_file:
            vid, title, _ = fetch_video_metadata(args.url, args.cookies_from_browser, args.cookies_file)
            meta = rewrite_summary(title, args.transcript_file, args.model, args.output_json, prompt_template)
        else:
            meta = process_video(
                args.url,
                use_whisper,
                args.model,
                args.output_json,
                prompt_template,
                args.cookies_from_browser,
                args.cookies_file,
            )
        # If no JSON output specified, print metadata as JSON to stdout
        if not args.output_json:
            print(json.dumps(meta, ensure_ascii=False, indent=2))
    except SystemExit as e:
        # Provide a friendly exit message without a stacktrace
        print(str(e))
        sys.exit(1)


if __name__ == '__main__':
    main()
