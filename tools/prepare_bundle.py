#!/usr/bin/env python3
"""
Prepare local bundle assets for a distributable Tauri build.

This script:
1. installs PyInstaller into the current Python environment
2. builds the bundled backend helper as a single executable
3. copies ffmpeg / ffprobe from the local PATH into Tauri resources

It targets the current host platform. Run it once per build machine before
`cargo tauri build`.
"""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SRC_TAURI = ROOT / "src-tauri"
BACKEND_ROOT = SRC_TAURI / "resources" / "backend"
FFMPEG_ROOT = SRC_TAURI / "resources" / "ffmpeg"
BUILD_DIR = ROOT / "build"
DIST_DIR = BUILD_DIR / "pyinstaller-dist"
WORK_DIR = BUILD_DIR / "pyinstaller-work"
SPEC_DIR = BUILD_DIR / "pyinstaller-spec"
BACKEND_NAME = "yts-backend"


def run(cmd: list[str]) -> None:
    subprocess.run(cmd, check=True, cwd=ROOT)


def detect_target_triple() -> str:
    try:
        output = subprocess.check_output(["rustc", "--print", "host-tuple"], text=True)
        return output.strip()
    except subprocess.CalledProcessError:
        verbose = subprocess.check_output(["rustc", "-Vv"], text=True)
        for line in verbose.splitlines():
            if line.startswith("host: "):
                return line.split(": ", 1)[1].strip()
    raise SystemExit("Unable to determine the Rust host target triple.")


def executable_suffix() -> str:
    return ".exe" if os.name == "nt" else ""


def ensure_pyinstaller() -> None:
    run([sys.executable, "-m", "pip", "install", "--upgrade", "pip"])
    run([sys.executable, "-m", "pip", "install", "-r", str(ROOT / "requirements.txt"), "pyinstaller"])


def build_backend_binary() -> Path:
    DIST_DIR.mkdir(parents=True, exist_ok=True)
    WORK_DIR.mkdir(parents=True, exist_ok=True)
    SPEC_DIR.mkdir(parents=True, exist_ok=True)

    cmd = [
        sys.executable,
        "-m",
        "PyInstaller",
        "--noconfirm",
        "--clean",
        "--onefile",
        "--name",
        BACKEND_NAME,
        "--distpath",
        str(DIST_DIR),
        "--workpath",
        str(WORK_DIR),
        "--specpath",
        str(SPEC_DIR),
        "--paths",
        str(ROOT),
        "--collect-submodules",
        "whisper",
        "--collect-submodules",
        "yt_dlp",
        "--collect-submodules",
        "youtube_transcript_api",
        "--collect-data",
        "whisper",
        "--collect-data",
        "yt_dlp",
        "--collect-data",
        "webvtt",
        "--collect-data",
        "youtube_transcript_api",
        str(ROOT / "backend_cli.py"),
    ]
    run(cmd)
    binary = DIST_DIR / f"{BACKEND_NAME}{executable_suffix()}"
    if not binary.exists():
        raise SystemExit(f"Expected backend binary was not produced: {binary}")
    return binary


def install_sidecar(binary: Path, target_triple: str) -> Path:
    target_dir = BACKEND_ROOT / target_triple
    target_dir.mkdir(parents=True, exist_ok=True)
    target = target_dir / f"{BACKEND_NAME}{executable_suffix()}"
    shutil.copy2(binary, target)
    if os.name != "nt":
        target.chmod(target.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return target


def resolve_tool_source(env_name: str, tool_name: str) -> Path:
    override = os.environ.get(env_name, "").strip()
    if override:
        return Path(override).expanduser().resolve()

    source = shutil.which(tool_name)
    if not source:
        raise SystemExit(
            f"Required build dependency not found: {tool_name}. "
            f"Put it on PATH or set {env_name}."
        )
    return Path(source).resolve()


def copy_tool_to_resources(env_name: str, tool_name: str, resource_dir: Path) -> Path:
    source_path = resolve_tool_source(env_name, tool_name)
    destination = resource_dir / source_path.name
    shutil.copy2(source_path, destination)
    if os.name != "nt":
        destination.chmod(destination.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return destination


def install_ffmpeg_resources(target_triple: str) -> tuple[Path, Path]:
    resource_dir = FFMPEG_ROOT / target_triple
    resource_dir.mkdir(parents=True, exist_ok=True)
    ffmpeg = copy_tool_to_resources("YTS_FFMPEG", "ffmpeg", resource_dir)
    ffprobe = copy_tool_to_resources("YTS_FFPROBE", "ffprobe", resource_dir)
    return ffmpeg, ffprobe


def main() -> int:
    target_triple = detect_target_triple()
    ensure_pyinstaller()
    backend_binary = build_backend_binary()
    sidecar = install_sidecar(backend_binary, target_triple)
    ffmpeg, ffprobe = install_ffmpeg_resources(target_triple)

    print(f"Prepared backend sidecar: {sidecar}")
    print(f"Prepared ffmpeg resource: {ffmpeg}")
    print(f"Prepared ffprobe resource: {ffprobe}")
    print("Next step: cargo tauri build")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
