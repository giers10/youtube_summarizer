#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    env,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, ErrorKind, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use open::that;
use reqwest::blocking::Client;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::menu::{MenuBuilder, SubmenuBuilder};
use tauri::{path::BaseDirectory, AppHandle, Emitter, Manager, State, WebviewWindow};

const DEFAULT_MODEL: &str = "mistral:latest";
const OLLAMA_TAGS_URL: &str = "http://localhost:11434/api/tags";
#[cfg(not(debug_assertions))]
const BACKEND_EXECUTABLE_NAME: &str = "yts-backend";
const DISCORD_MAX_MESSAGE_LENGTH: usize = 2000;
const DISCORD_MESSAGE_DELAY_MS: u64 = 1000;
const TARGET_TRIPLE: &str = env!("TAURI_BUILD_TARGET");
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone)]
enum BackendRuntime {
    Bundled {
        executable: PathBuf,
    },
    Python {
        python: PathBuf,
        script_dir: PathBuf,
    },
}

#[derive(Clone)]
struct AppState {
    app_dir: PathBuf,
    media_dir: PathBuf,
    db_path: PathBuf,
    backend_log_path: PathBuf,
    backend: BackendRuntime,
    ffmpeg_path: Option<PathBuf>,
    ffprobe_path: Option<PathBuf>,
    whisper_cache_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SummarizeVideoRequest {
    url: String,
    use_whisper: bool,
    model: Option<String>,
    master_prompt: Option<String>,
    cookie_source: Option<YoutubeCookieSourceRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct YoutubeCookieSourceRequest {
    source_type: String,
    browser: Option<String>,
    profile: Option<String>,
    keyring: Option<String>,
    container: Option<String>,
    cookies_file: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeleteSummaryRequest {
    id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranslateSummaryRequest {
    id: i64,
    lang: String,
    model: Option<String>,
    prompt_template: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendSummaryToDiscordRequest {
    id: i64,
    webhook_url: String,
}

#[derive(Debug, Deserialize)]
struct BackendSummaryMeta {
    timestamp: String,
    video_id: String,
    url: String,
    video_name: String,
    channel: Option<String>,
    thumbnail: Option<String>,
    audio: Option<String>,
    transcript: Option<String>,
    summary: String,
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModel>,
}

#[derive(Debug, Deserialize)]
struct OllamaModel {
    name: String,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct YoutubeCookieBrowserOption {
    id: &'static str,
    label: &'static str,
}

const YOUTUBE_COOKIE_BROWSER_OPTIONS: &[YoutubeCookieBrowserOption] = &[
    YoutubeCookieBrowserOption {
        id: "chrome",
        label: "Google Chrome",
    },
    YoutubeCookieBrowserOption {
        id: "firefox",
        label: "Firefox",
    },
    YoutubeCookieBrowserOption {
        id: "safari",
        label: "Safari",
    },
    YoutubeCookieBrowserOption {
        id: "edge",
        label: "Microsoft Edge",
    },
    YoutubeCookieBrowserOption {
        id: "brave",
        label: "Brave",
    },
    YoutubeCookieBrowserOption {
        id: "chromium",
        label: "Chromium",
    },
    YoutubeCookieBrowserOption {
        id: "vivaldi",
        label: "Vivaldi",
    },
    YoutubeCookieBrowserOption {
        id: "opera",
        label: "Opera",
    },
    YoutubeCookieBrowserOption {
        id: "whale",
        label: "Naver Whale",
    },
];

#[derive(Debug)]
struct StoredSummary {
    id: i64,
    timestamp: Option<String>,
    video_id: Option<String>,
    url: Option<String>,
    video_name: Option<String>,
    channel: Option<String>,
    thumbnail: Option<String>,
    audio: Option<String>,
    transcript: Option<String>,
    summary_en: Option<String>,
    summary_de: Option<String>,
    summary_jp: Option<String>,
}

#[derive(Debug, Serialize)]
struct SummaryEntry {
    id: i64,
    timestamp: Option<String>,
    video_id: Option<String>,
    url: Option<String>,
    video_name: Option<String>,
    channel: Option<String>,
    thumbnail: Option<String>,
    audio: Option<String>,
    transcript: Option<String>,
    summary_en: Option<String>,
    summary_de: Option<String>,
    summary_jp: Option<String>,
}

impl StoredSummary {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            timestamp: row.get("timestamp")?,
            video_id: row.get("video_id")?,
            url: row.get("url")?,
            video_name: row.get("video_name")?,
            channel: row.get("channel")?,
            thumbnail: row.get("thumbnail")?,
            audio: row.get("audio")?,
            transcript: row.get("transcript")?,
            summary_en: row.get("summary_en")?,
            summary_de: row.get("summary_de")?,
            summary_jp: row.get("summary_jp")?,
        })
    }

    fn into_entry(self, state: &AppState) -> SummaryEntry {
        SummaryEntry {
            id: self.id,
            timestamp: self.timestamp,
            video_id: self.video_id,
            url: self.url,
            video_name: self.video_name,
            channel: self.channel,
            thumbnail: absolute_media_path(state, self.thumbnail),
            audio: absolute_media_path(state, self.audio),
            transcript: absolute_media_path(state, self.transcript),
            summary_en: self.summary_en,
            summary_de: self.summary_de,
            summary_jp: self.summary_jp,
        }
    }
}

fn absolute_media_path(state: &AppState, file_name: Option<String>) -> Option<String> {
    file_name.map(|name| state.media_dir.join(name).to_string_lossy().into_owned())
}

fn normalize_model(model: Option<String>) -> String {
    model
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

fn normalize_prompt_template(prompt: Option<String>) -> Option<String> {
    prompt
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn clean_optional_string(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn is_supported_cookie_browser(browser: &str) -> bool {
    YOUTUBE_COOKIE_BROWSER_OPTIONS
        .iter()
        .any(|option| option.id == browser)
}

fn is_supported_linux_keyring(keyring: &str) -> bool {
    matches!(
        keyring,
        "basictext" | "gnomekeyring" | "kwallet" | "kwallet5" | "kwallet6"
    )
}

fn expand_user_path(raw_path: &str) -> PathBuf {
    if let Some(rest) = raw_path.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME").or_else(|_| env::var("USERPROFILE")) {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(raw_path)
}

fn build_youtube_cookie_args(
    cookie_source: Option<YoutubeCookieSourceRequest>,
) -> Result<Vec<String>, String> {
    let Some(source) = cookie_source else {
        return Ok(Vec::new());
    };

    match source.source_type.trim() {
        "browser" => {
            let browser = clean_optional_string(&source.browser)
                .map(|value| value.to_ascii_lowercase())
                .ok_or_else(|| "Select a browser cookie source.".to_string())?;
            if !is_supported_cookie_browser(&browser) {
                return Err(format!("Unsupported browser cookie source: {browser}"));
            }

            let mut spec = browser;
            if let Some(keyring) = clean_optional_string(&source.keyring)
                .map(|value| value.to_ascii_lowercase())
            {
                if !is_supported_linux_keyring(&keyring) {
                    return Err(format!("Unsupported Linux keyring: {keyring}"));
                }
                spec.push('+');
                spec.push_str(&keyring);
            }
            if let Some(profile) = clean_optional_string(&source.profile) {
                spec.push(':');
                spec.push_str(&profile);
            }
            if let Some(container) = clean_optional_string(&source.container) {
                spec.push_str("::");
                spec.push_str(&container);
            }
            Ok(vec!["--cookies-from-browser".to_string(), spec])
        }
        "cookiesFile" => {
            let cookies_file = clean_optional_string(&source.cookies_file)
                .ok_or_else(|| "Enter a cookies.txt file path.".to_string())?;
            let path = expand_user_path(&cookies_file);
            if !path.exists() {
                return Err(format!("Cookies file does not exist: {}", path.display()));
            }
            Ok(vec![
                "--cookies-file".to_string(),
                path.to_string_lossy().into_owned(),
            ])
        }
        "" => Ok(Vec::new()),
        other => Err(format!("Unsupported YouTube cookie source type: {other}")),
    }
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn get_youtube_cookie_browser_options_inner() -> Vec<YoutubeCookieBrowserOption> {
    YOUTUBE_COOKIE_BROWSER_OPTIONS
        .iter()
        .copied()
        .filter(|option| browser_is_available(option.id))
        .collect()
}

#[cfg(target_os = "macos")]
fn browser_is_available(id: &str) -> bool {
    let app_names: &[&str] = match id {
        "brave" => &["Brave Browser.app"],
        "chrome" => &["Google Chrome.app"],
        "chromium" => &["Chromium.app"],
        "edge" => &["Microsoft Edge.app"],
        "firefox" => &["Firefox.app"],
        "opera" => &["Opera.app"],
        "safari" => &["Safari.app"],
        "vivaldi" => &["Vivaldi.app"],
        "whale" => &["Whale.app", "Naver Whale.app"],
        _ => return false,
    };

    let mut roots = vec![PathBuf::from("/Applications")];
    if let Ok(home) = env::var("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }

    roots
        .iter()
        .any(|root| app_names.iter().any(|name| root.join(name).exists()))
}

#[cfg(target_os = "windows")]
fn browser_is_available(id: &str) -> bool {
    windows_browser_paths(id).iter().any(|path| path.exists())
}

#[cfg(target_os = "windows")]
fn windows_browser_paths(id: &str) -> Vec<PathBuf> {
    let program_files = env::var("ProgramFiles").ok().map(PathBuf::from);
    let program_files_x86 = env::var("ProgramFiles(x86)").ok().map(PathBuf::from);
    let local_app_data = env::var("LocalAppData").ok().map(PathBuf::from);
    let mut paths = Vec::new();

    let mut add_under_program_files = |relative: &str| {
        if let Some(root) = &program_files {
            paths.push(root.join(relative));
        }
        if let Some(root) = &program_files_x86 {
            paths.push(root.join(relative));
        }
    };
    let mut add_under_local_app_data = |relative: &str| {
        if let Some(root) = &local_app_data {
            paths.push(root.join(relative));
        }
    };

    match id {
        "brave" => {
            add_under_program_files("BraveSoftware/Brave-Browser/Application/brave.exe");
            add_under_local_app_data("BraveSoftware/Brave-Browser/Application/brave.exe");
        }
        "chrome" => {
            add_under_program_files("Google/Chrome/Application/chrome.exe");
            add_under_local_app_data("Google/Chrome/Application/chrome.exe");
        }
        "chromium" => {
            add_under_program_files("Chromium/Application/chrome.exe");
            add_under_local_app_data("Chromium/Application/chrome.exe");
        }
        "edge" => {
            add_under_program_files("Microsoft/Edge/Application/msedge.exe");
            add_under_local_app_data("Microsoft/Edge/Application/msedge.exe");
        }
        "firefox" => {
            add_under_program_files("Mozilla Firefox/firefox.exe");
            add_under_local_app_data("Mozilla Firefox/firefox.exe");
        }
        "opera" => {
            add_under_local_app_data("Programs/Opera/opera.exe");
            add_under_program_files("Opera/opera.exe");
        }
        "vivaldi" => {
            add_under_program_files("Vivaldi/Application/vivaldi.exe");
            add_under_local_app_data("Vivaldi/Application/vivaldi.exe");
        }
        "whale" => {
            add_under_program_files("Naver/Naver Whale/Application/whale.exe");
            add_under_local_app_data("Naver/Naver Whale/Application/whale.exe");
        }
        _ => {}
    }

    paths
}

#[cfg(target_os = "linux")]
fn browser_is_available(id: &str) -> bool {
    let commands: &[&str] = match id {
        "brave" => &["brave-browser", "brave"],
        "chrome" => &["google-chrome", "google-chrome-stable", "chrome"],
        "chromium" => &["chromium", "chromium-browser"],
        "edge" => &["microsoft-edge", "microsoft-edge-stable"],
        "firefox" => &["firefox"],
        "opera" => &["opera"],
        "vivaldi" => &["vivaldi", "vivaldi-stable"],
        "whale" => &["whale"],
        "safari" => &[],
        _ => &[],
    };
    commands.iter().any(|command| command_exists(command))
}

#[cfg(target_os = "linux")]
fn command_exists(command: &str) -> bool {
    if command.contains(std::path::MAIN_SEPARATOR) {
        return Path::new(command).exists();
    }
    let Some(paths) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&paths).any(|path| path.join(command).exists())
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn browser_is_available(_id: &str) -> bool {
    false
}

fn resolve_project_root() -> Result<PathBuf, String> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .map_err(|err| format!("Failed to resolve project root: {err}"))
}

fn platform_executable_name(base_name: &str) -> String {
    if cfg!(windows) {
        format!("{base_name}.exe")
    } else {
        base_name.to_string()
    }
}

fn resolve_resource_file(app: &AppHandle, relative_path: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(resource_path) = app.path().resolve(relative_path, BaseDirectory::Resource) {
        candidates.push(resource_path);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(manifest_dir.join(relative_path));
    candidates.push(manifest_dir.join("resources").join(relative_path));
    if let Ok(project_root) = resolve_project_root() {
        candidates.push(project_root.join(relative_path));
    }

    candidates.into_iter().find(|path| path.exists())
}

fn resolve_backend_override() -> Option<PathBuf> {
    if let Ok(path) = env::var("YTS_BACKEND_BIN") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    None
}

#[cfg(not(debug_assertions))]
fn resolve_bundled_backend_binary(app: &AppHandle) -> Option<PathBuf> {
    let relative_path = Path::new("backend")
        .join(TARGET_TRIPLE)
        .join(platform_executable_name(BACKEND_EXECUTABLE_NAME));
    resolve_resource_file(app, &relative_path)
}

fn resolve_script_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(resource_file) = resolve_resource_file(app, Path::new("backend_cli.py")) {
        if let Some(parent) = resource_file.parent() {
            return Ok(parent.to_path_buf());
        }
    }

    let project_dir = resolve_project_root()?;
    if project_dir.join("backend_cli.py").exists() {
        return Ok(project_dir);
    }

    Err("Unable to locate bundled or development backend Python scripts.".to_string())
}

fn resolve_python_command(script_dir: &Path) -> Result<PathBuf, String> {
    if let Ok(path) = env::var("YTS_PYTHON") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }

    let mut candidates = Vec::new();
    candidates.push(script_dir.join("venv").join("bin").join("python3"));
    candidates.push(script_dir.join("venv").join("bin").join("python"));
    candidates.push(script_dir.join("venv").join("Scripts").join("python.exe"));
    candidates.push(PathBuf::from("python3"));
    candidates.push(PathBuf::from("python"));

    for candidate in candidates {
        if Command::new(&candidate).arg("--version").output().is_ok() {
            return Ok(candidate);
        }
    }

    Err("Unable to find a usable Python interpreter. Set YTS_PYTHON to override.".to_string())
}

fn resolve_backend_runtime(app: &AppHandle) -> Result<BackendRuntime, String> {
    if let Some(executable) = resolve_backend_override() {
        return Ok(BackendRuntime::Bundled { executable });
    }

    #[cfg(debug_assertions)]
    {
        let script_dir = resolve_script_dir(app)?;
        let python = resolve_python_command(&script_dir)?;
        Ok(BackendRuntime::Python { python, script_dir })
    }

    #[cfg(not(debug_assertions))]
    {
        if let Some(executable) = resolve_bundled_backend_binary(app) {
            return Ok(BackendRuntime::Bundled { executable });
        }

        let script_dir = resolve_script_dir(app)?;
        let python = resolve_python_command(&script_dir)?;
        Ok(BackendRuntime::Python { python, script_dir })
    }
}

fn resolve_optional_tool_path(app: &AppHandle, env_name: &str, tool_name: &str) -> Option<PathBuf> {
    if let Ok(path) = env::var(env_name) {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    let relative_path = Path::new("ffmpeg")
        .join(TARGET_TRIPLE)
        .join(platform_executable_name(tool_name));
    resolve_resource_file(app, &relative_path)
}

fn resolve_whisper_cache_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let cache_root = app
        .path()
        .app_cache_dir()
        .or_else(|_| app.path().app_local_data_dir())
        .map_err(|err| format!("Failed to resolve application cache directory: {err}"))?;
    let whisper_cache_dir = cache_root.join("whisper");
    fs::create_dir_all(&whisper_cache_dir)
        .map_err(|err| format!("Failed to create Whisper cache directory: {err}"))?;
    Ok(whisper_cache_dir)
}

fn resolve_log_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .or_else(|_| {
            app.path()
                .app_local_data_dir()
                .map(|path| path.join("logs"))
        })
        .map_err(|err| format!("Failed to resolve application log directory: {err}"))?;
    fs::create_dir_all(&log_dir)
        .map_err(|err| format!("Failed to create application log directory: {err}"))?;
    Ok(log_dir)
}

fn open_connection(state: &AppState) -> Result<Connection, String> {
    Connection::open(&state.db_path).map_err(|err| format!("Failed to open SQLite database: {err}"))
}

fn init_db(state: &AppState) -> Result<(), String> {
    let db = open_connection(state)?;
    db.execute_batch(
        r#"
      CREATE TABLE IF NOT EXISTS summaries (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        timestamp TEXT,
        video_id TEXT,
        url TEXT,
        video_name TEXT,
        channel TEXT,
        thumbnail TEXT,
        audio TEXT,
        transcript TEXT,
        summary_en TEXT,
        summary_de TEXT,
        summary_jp TEXT
      );
    "#,
    )
    .map_err(|err| format!("Failed to initialize SQLite schema: {err}"))?;
    Ok(())
}

fn remove_named_media_file(media_dir: &Path, file_name: &str) {
    let path = media_dir.join(file_name);
    if let Err(err) = fs::remove_file(&path) {
        if err.kind() != ErrorKind::NotFound {
            eprintln!("Failed to remove {}: {}", path.display(), err);
        }
    }
}

fn cleanup_artifacts(state: &AppState, audio: Option<&str>, transcript: Option<&str>) {
    if let Some(audio_file) = audio.filter(|value| !value.trim().is_empty()) {
        remove_named_media_file(&state.media_dir, audio_file);
    }
    if let Some(transcript_file) = transcript.filter(|value| !value.trim().is_empty()) {
        remove_named_media_file(&state.media_dir, transcript_file);
    }
}

fn purge_existing_artifacts(state: &AppState) -> Result<(), String> {
    let db = open_connection(state)?;
    let mut stmt = db
        .prepare(
            "SELECT id, audio, transcript FROM summaries WHERE audio IS NOT NULL OR transcript IS NOT NULL",
        )
        .map_err(|err| format!("Failed to prepare artifact cleanup query: {err}"))?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|err| format!("Failed to load stored artifacts: {err}"))?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row.map_err(|err| format!("Failed to decode stored artifact row: {err}"))?);
    }
    drop(stmt);

    for (id, audio, transcript) in entries {
        cleanup_artifacts(state, audio.as_deref(), transcript.as_deref());
        db.execute(
            "UPDATE summaries SET audio = NULL, transcript = NULL WHERE id = ?",
            [id],
        )
        .map_err(|err| format!("Failed to clear stored artifact references: {err}"))?;
    }

    Ok(())
}

fn write_startup_error_log(app: &AppHandle, message: &str) {
    let mut candidates = Vec::new();

    if let Ok(path) = app.path().app_log_dir() {
        candidates.push(path);
    }
    if let Ok(path) = app.path().app_local_data_dir() {
        candidates.push(path);
    }
    candidates.push(env::temp_dir().join("youtube-summarizer"));

    for directory in candidates {
        if fs::create_dir_all(&directory).is_ok() {
            let log_path = directory.join("startup-error.log");
            if fs::write(&log_path, message).is_ok() {
                eprintln!("Startup failure written to {}", log_path.display());
                return;
            }
        }
    }

    eprintln!("{message}");
}

fn ensure_app_state(app: &AppHandle) -> Result<AppState, String> {
    let app_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|err| format!("Failed to resolve application data directory: {err}"))?;
    let media_dir = app_dir.join("data");
    fs::create_dir_all(&media_dir)
        .map_err(|err| format!("Failed to create application data directory: {err}"))?;

    let state = AppState {
        backend: resolve_backend_runtime(app)?,
        ffmpeg_path: resolve_optional_tool_path(app, "YTS_FFMPEG", "ffmpeg"),
        ffprobe_path: resolve_optional_tool_path(app, "YTS_FFPROBE", "ffprobe"),
        whisper_cache_dir: resolve_whisper_cache_dir(app)?,
        backend_log_path: resolve_log_dir(app)?.join("backend.log"),
        app_dir: app_dir.clone(),
        media_dir,
        db_path: app_dir.join("summaries.db"),
    };

    init_db(&state)?;
    purge_existing_artifacts(&state)?;
    Ok(state)
}

fn emit_progress(app: &AppHandle, window_label: &str, line: &str) {
    let trimmed = line.trim();
    if !trimmed.is_empty() {
        let _ = app.emit_to(window_label, "summarize-progress", trimmed.to_string());
    }
}

fn append_backend_log(log_path: &Path, line: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(file, "{line}");
    }
}

fn backend_failure_message(stderr_output: &str, fallback: String) -> String {
    for line in stderr_output.lines().rev() {
        let trimmed = line.trim();
        if let Some(message) = trimmed.strip_prefix("[error]") {
            let message = message.trim();
            if !message.is_empty() {
                return message.to_string();
            }
        }
    }

    for line in stderr_output.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("WARNING:")
            || trimmed.starts_with("Traceback")
            || trimmed.starts_with("File ")
            || trimmed.starts_with("During handling")
        {
            continue;
        }
        if trimmed.starts_with("ERROR:")
            || trimmed.contains("RuntimeError:")
            || trimmed.contains("SystemExit:")
        {
            return trimmed.to_string();
        }
    }

    fallback
}

fn apply_backend_env(command: &mut Command, state: &AppState) {
    command.env("PYTHONUNBUFFERED", "1");
    command.env("PYTHONIOENCODING", "utf-8");
    command.env("YTS_WHISPER_CACHE_DIR", &state.whisper_cache_dir);

    if let Some(ffmpeg_path) = &state.ffmpeg_path {
        command.env("YTS_FFMPEG", ffmpeg_path);
    }
    if let Some(ffprobe_path) = &state.ffprobe_path {
        command.env("YTS_FFPROBE", ffprobe_path);
    }
}

fn build_backend_command(state: &AppState, args: &[String]) -> Command {
    let mut command = match &state.backend {
        BackendRuntime::Bundled { executable } => Command::new(executable),
        BackendRuntime::Python { python, script_dir } => {
            let mut command = Command::new(python);
            command.arg(script_dir.join("backend_cli.py"));
            command
        }
    };

    command.args(args).current_dir(&state.media_dir);
    apply_backend_env(&mut command, state);
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn run_backend_json_command(
    state: &AppState,
    app: &AppHandle,
    window_label: &str,
    args: &[String],
) -> Result<BackendSummaryMeta, String> {
    let output_path = state.app_dir.join(format!("tmp_{}.json", now_millis()));
    let mut command_args = args.to_vec();
    command_args.push("--output-json".to_string());
    command_args.push(output_path.to_string_lossy().into_owned());
    append_backend_log(
        &state.backend_log_path,
        &format!("=== summarize {} ===", command_args.join(" ")),
    );

    let mut child = build_backend_command(state, &command_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            let message = format!("Failed to start bundled backend: {err}");
            append_backend_log(&state.backend_log_path, &message);
            message
        })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Backend stdout was not captured.".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Backend stderr was not captured.".to_string())?;
    let stderr_buffer = Arc::new(Mutex::new(String::new()));
    let stdout_log_path = state.backend_log_path.clone();
    let stderr_log_path = state.backend_log_path.clone();

    let stdout_app = app.clone();
    let stdout_label = window_label.to_string();
    let stdout_handle = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    append_backend_log(&stdout_log_path, &format!("[stdout] {line}"));
                    emit_progress(&stdout_app, &stdout_label, &line);
                }
                Err(_) => break,
            }
        }
    });

    let stderr_buffer_clone = Arc::clone(&stderr_buffer);
    let stderr_handle = thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            match line {
                Ok(line) => {
                    append_backend_log(&stderr_log_path, &format!("[stderr] {line}"));
                    if let Ok(mut buffer) = stderr_buffer_clone.lock() {
                        buffer.push_str(&line);
                        buffer.push('\n');
                    }
                }
                Err(_) => break,
            }
        }
    });

    let status = child.wait().map_err(|err| {
        let message = format!("Failed to wait for bundled backend: {err}");
        append_backend_log(&state.backend_log_path, &message);
        message
    })?;
    append_backend_log(
        &state.backend_log_path,
        &format!("Bundled backend exit status: {status}"),
    );

    let _ = stdout_handle.join();
    let _ = stderr_handle.join();

    if !status.success() {
        let stderr_output = stderr_buffer
            .lock()
            .map(|buffer| buffer.trim().to_string())
            .unwrap_or_else(|_| String::new());
        let message = backend_failure_message(
            &stderr_output,
            format!("Bundled backend exited with status {status}."),
        );
        append_backend_log(
            &state.backend_log_path,
            &format!("Backend failure: {message}"),
        );
        let _ = fs::remove_file(&output_path);
        return Err(message);
    }

    let raw_json = fs::read_to_string(&output_path).map_err(|err| {
        let message = format!("Failed to read backend output JSON: {err}");
        append_backend_log(&state.backend_log_path, &message);
        message
    })?;
    let _ = fs::remove_file(&output_path);

    serde_json::from_str(&raw_json).map_err(|err| {
        let message = format!("Invalid backend output JSON: {err}");
        append_backend_log(&state.backend_log_path, &message);
        message
    })
}

fn run_backend_text_command(state: &AppState, args: &[String]) -> Result<String, String> {
    append_backend_log(
        &state.backend_log_path,
        &format!("=== translate {} ===", args.join(" ")),
    );
    let output = build_backend_command(state, args).output().map_err(|err| {
        let message = format!("Failed to start translation backend: {err}");
        append_backend_log(&state.backend_log_path, &message);
        message
    })?;

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        append_backend_log(&state.backend_log_path, &format!("[stdout] {line}"));
    }
    for line in String::from_utf8_lossy(&output.stderr).lines() {
        append_backend_log(&state.backend_log_path, &format!("[stderr] {line}"));
    }
    append_backend_log(
        &state.backend_log_path,
        &format!("Translation backend exit status: {}", output.status),
    );

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            format!("Translation backend exited with status {}.", output.status)
        } else {
            stderr
        };
        append_backend_log(
            &state.backend_log_path,
            &format!("Translation failure: {message}"),
        );
        return Err(message);
    }

    let translation = String::from_utf8(output.stdout)
        .map_err(|err| format!("Translation backend returned invalid UTF-8: {err}"))?
        .trim()
        .to_string();
    if translation.is_empty() {
        let message = "Translation backend returned an empty result.".to_string();
        append_backend_log(&state.backend_log_path, &message);
        return Err(message);
    }

    Ok(translation)
}

fn get_entry_by_id(state: &AppState, id: i64) -> Result<SummaryEntry, String> {
    let db = open_connection(state)?;
    let stored = db
        .query_row(
            "SELECT * FROM summaries WHERE id = ?",
            [id],
            StoredSummary::from_row,
        )
        .optional()
        .map_err(|err| format!("Failed to query summary entry: {err}"))?
        .ok_or_else(|| "Entry not found.".to_string())?;
    Ok(stored.into_entry(state))
}

fn option_trimmed(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn select_summary_for_discord(entry: &SummaryEntry) -> Result<&str, String> {
    option_trimmed(&entry.summary_de)
        .or_else(|| option_trimmed(&entry.summary_en))
        .or_else(|| option_trimmed(&entry.summary_jp))
        .ok_or_else(|| "Entry has no summary to send.".to_string())
}

fn parse_discord_webhook_url(raw_url: &str) -> Result<reqwest::Url, String> {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() {
        return Err("Add a Discord Webhook URL in Settings first.".to_string());
    }

    let url =
        reqwest::Url::parse(trimmed).map_err(|_| "Discord Webhook URL is invalid.".to_string())?;
    if url.scheme() != "https" {
        return Err("Discord Webhook URL must use HTTPS.".to_string());
    }

    let host = url.host_str().unwrap_or_default();
    let allowed_host = matches!(
        host,
        "discord.com" | "discordapp.com" | "canary.discord.com" | "ptb.discord.com"
    );
    if !allowed_host || !url.path().starts_with("/api/webhooks/") {
        return Err("Discord Webhook URL must be a Discord webhook URL.".to_string());
    }

    Ok(url)
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

fn take_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn skip_chars(text: &str, chars_to_skip: usize) -> String {
    text.chars().skip(chars_to_skip).collect()
}

fn limit_chars(text: &str, max_chars: usize) -> String {
    if char_len(text) <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return take_chars(text, max_chars);
    }
    format!("{}...", take_chars(text, max_chars - 3))
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn find_first_tag(text: &str, tags: &[&str]) -> Option<(usize, usize)> {
    tags.iter()
        .filter_map(|tag| text.find(tag).map(|index| (index, tag.len())))
        .min_by_key(|(index, _)| *index)
}

fn remove_think_blocks(text: &str) -> String {
    let mut output = String::new();
    let mut rest = text;

    while let Some((start, tag_len)) = find_first_tag(rest, &["<think>", "<thinking>"]) {
        output.push_str(&rest[..start]);
        let after_start = &rest[start + tag_len..];
        if let Some((end, end_tag_len)) = find_first_tag(after_start, &["</think>", "</thinking>"])
        {
            rest = &after_start[end + end_tag_len..];
        } else {
            rest = "";
            break;
        }
    }

    output.push_str(rest);
    output
}

fn is_sentence_start(ch: char) -> bool {
    ch.is_ascii_uppercase() || ch.is_ascii_digit() || matches!(ch, '"' | '\'' | '(' | '[')
}

fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '.'
}

fn is_abbreviation(token: &str) -> bool {
    const ABBREVIATIONS: &[&str] = &[
        "mr.", "mrs.", "ms.", "dr.", "prof.", "sr.", "jr.", "st.", "vs.", "etc.", "e.g.", "i.e.",
        "cf.", "al.", "a.m.", "p.m.", "jan.", "feb.", "mar.", "apr.", "jun.", "jul.", "aug.",
        "sep.", "sept.", "oct.", "nov.", "dec.", "no.", "fig.", "eq.", "vol.", "rev.", "gen.",
        "gov.", "sen.", "rep.", "dept.", "univ.", "inc.", "ltd.", "co.", "corp.", "bros.",
        "approx.", "est.", "min.", "sec.", "hr.", "fr.", "bzw.", "z.b.", "d.h.", "u.a.", "u.u.",
        "i.d.r.", "ca.", "ggf.", "vgl.", "evtl.", "sog.", "u.s.w.", "nr.", "abs.", "art.", "s.",
        "ff.", "mio.", "mrd.", "sek.", "okt.", "dez.",
    ];
    ABBREVIATIONS.contains(&token)
}

fn is_non_terminal_punctuation_token(token: &str) -> bool {
    matches!(token, "yahoo!" | "jeopardy!" | "wii!" | "o2!" | "who?")
}

fn insert_sentence_line_breaks(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut output = String::new();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        output.push(ch);

        if !matches!(ch, '.' | '!' | '?') {
            i += 1;
            continue;
        }

        let Some(next) = chars.get(i + 1) else {
            i += 1;
            continue;
        };
        if !next.is_whitespace() {
            i += 1;
            continue;
        }

        let mut k = i + 1;
        while k < chars.len() && chars[k].is_whitespace() {
            k += 1;
        }
        if k >= chars.len() || !is_sentence_start(chars[k]) {
            i += 1;
            continue;
        }

        if ch == '.'
            && i > 0
            && chars[i - 1].is_ascii_alphabetic()
            && chars.get(k + 1) == Some(&'.')
        {
            i += 1;
            continue;
        }

        let mut j = i;
        while j > 0 && is_word_char(chars[j - 1]) {
            j -= 1;
        }
        let token = chars[j..=i].iter().collect::<String>().to_ascii_lowercase();
        if ch == '.' && is_abbreviation(&token) {
            i += 1;
            continue;
        }
        if matches!(ch, '!' | '?') && is_non_terminal_punctuation_token(&token) {
            i += 1;
            continue;
        }

        output.push('\n');
        i = k;
    }

    output
}

fn parse_markdown_heading(line: &str) -> Option<&str> {
    let hash_count = line.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&hash_count) {
        return None;
    }
    line.get(hash_count..)
        .and_then(|rest| rest.strip_prefix(' '))
        .map(str::trim)
        .filter(|heading| !heading.is_empty())
}

fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    if char_len(trimmed) < 3 {
        return false;
    }
    trimmed.chars().all(|ch| ch == '-')
        || trimmed.chars().all(|ch| ch == '*')
        || trimmed.chars().all(|ch| ch == '_')
}

fn parse_numbered_list(line: &str) -> Option<(&str, &str)> {
    let (number, rest) = line.split_once(". ")?;
    if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some((number, rest.trim()))
}

fn parse_bullet_list(line: &str) -> Option<&str> {
    line.strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
        .map(str::trim)
}

fn collapse_inline_whitespace(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn collapse_blank_lines(text: &str) -> String {
    let mut lines = Vec::new();
    let mut blank_count = 0;

    for line in text.lines() {
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                lines.push(String::new());
            }
        } else {
            blank_count = 0;
            lines.push(line.to_string());
        }
    }

    lines.join("\n").trim().to_string()
}

fn format_summary_for_discord(summary_text: &str) -> String {
    let mut text = normalize_newlines(&remove_think_blocks(summary_text));
    if !text.contains('\n') && !text.contains("```") {
        text = insert_sentence_line_breaks(&text);
    }

    let mut formatted = Vec::new();
    let mut in_code_block = false;
    let mut in_ordered_context = false;

    for raw_line in text.lines() {
        let trimmed_line = raw_line.trim_end();
        let compact_line = trimmed_line.trim();

        if compact_line.starts_with("```") {
            in_code_block = !in_code_block;
            formatted.push(trimmed_line.to_string());
            continue;
        }
        if in_code_block {
            formatted.push(trimmed_line.to_string());
            continue;
        }
        if compact_line.is_empty() {
            formatted.push(String::new());
            in_ordered_context = false;
            continue;
        }

        if let Some(heading) = parse_markdown_heading(compact_line) {
            formatted.push(format!("**{heading}**"));
            formatted.push(String::new());
            in_ordered_context = false;
            continue;
        }
        if is_horizontal_rule(compact_line) {
            formatted.push(String::new());
            in_ordered_context = false;
            continue;
        }
        if let Some((number, rest)) = parse_numbered_list(compact_line) {
            formatted.push(format!("{number}. {rest}"));
            in_ordered_context = true;
            continue;
        }
        if let Some(rest) = parse_bullet_list(compact_line) {
            let indent = if in_ordered_context { "  " } else { "" };
            formatted.push(format!("{indent}- {rest}"));
            continue;
        }

        formatted.push(collapse_inline_whitespace(compact_line));
        in_ordered_context = false;
    }

    collapse_blank_lines(&formatted.join("\n"))
}

fn split_segment_by_words(segment: &str, max_len: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut piece = String::new();

    for word in segment.split(' ') {
        let next = if piece.is_empty() {
            word.to_string()
        } else {
            format!("{piece} {word}")
        };

        if char_len(&next) <= max_len {
            piece = next;
            continue;
        }

        if !piece.is_empty() {
            chunks.push(piece.trim_end().to_string());
        }

        piece = word.to_string();
        while char_len(&piece) > max_len {
            chunks.push(take_chars(&piece, max_len));
            piece = skip_chars(&piece, max_len);
        }
    }

    if !piece.is_empty() {
        chunks.push(piece.trim_end().to_string());
    }

    chunks
}

fn split_summary_into_chunks(summary_text: &str, max_len: usize) -> Vec<String> {
    let cleaned = normalize_newlines(summary_text).trim().to_string();
    if cleaned.is_empty() {
        return vec![String::new()];
    }

    let prepared = if cleaned.contains('\n') {
        cleaned
    } else {
        insert_sentence_line_breaks(&cleaned)
    };

    let mut segments = Vec::new();
    let mut lines = prepared.split('\n').peekable();
    while let Some(line) = lines.next() {
        segments.push(line.to_string());
        if lines.peek().is_some() {
            segments.push("\n".to_string());
        }
    }

    let mut chunks = Vec::new();
    let mut current = String::new();

    for segment in segments {
        let candidate = format!("{current}{segment}");
        if char_len(&candidate) <= max_len {
            current = candidate;
            continue;
        }

        if !current.is_empty() {
            chunks.push(current.trim_end().to_string());
            current.clear();
        }

        if char_len(&segment) <= max_len {
            current = segment;
        } else {
            chunks.extend(split_segment_by_words(&segment, max_len));
        }
    }

    if !current.is_empty() {
        chunks.push(current.trim_end().to_string());
    }

    if chunks.is_empty() {
        vec![String::new()]
    } else {
        chunks
    }
}

fn build_discord_messages(
    title_line: &str,
    channel_text_line: &str,
    video_line: &str,
    summary_text: &str,
) -> Vec<String> {
    let first_prefix = format!("{title_line}\n{channel_text_line}\n\n");
    let last_suffix = format!("\n\n{video_line}");

    let available = |used: usize| DISCORD_MAX_MESSAGE_LENGTH.saturating_sub(used).max(1);
    let first_limit = available(char_len(&first_prefix));
    let middle_limit = DISCORD_MAX_MESSAGE_LENGTH;
    let last_limit = available(char_len(&last_suffix));
    let single_limit = available(char_len(&first_prefix) + char_len(&last_suffix));

    let mut chunks = split_summary_into_chunks(summary_text, middle_limit);
    if chunks.len() == 1 && char_len(&chunks[0]) > single_limit {
        chunks = split_summary_into_chunks(summary_text, first_limit);
    }

    if char_len(&chunks[0]) > first_limit {
        let first_parts = split_summary_into_chunks(&chunks[0], first_limit);
        chunks.splice(0..1, first_parts);
    }

    if chunks.len() > 1 && chunks.last().map(|chunk| char_len(chunk)).unwrap_or(0) > last_limit {
        let last_index = chunks.len() - 1;
        let last_parts = split_summary_into_chunks(&chunks[last_index], last_limit);
        chunks.splice(last_index.., last_parts);
    }

    if chunks.len() == 1 {
        return vec![format!("{first_prefix}{}{last_suffix}", chunks[0])];
    }

    let mut messages = Vec::new();
    messages.push(format!("{first_prefix}{}", chunks[0]));
    for chunk in chunks.iter().skip(1).take(chunks.len().saturating_sub(2)) {
        messages.push(chunk.to_string());
    }
    messages.push(format!("{}{last_suffix}", chunks[chunks.len() - 1]));
    messages
}

fn fetch_channel_name(client: &Client, video_url: Option<&str>) -> String {
    let Some(video_url) = video_url.map(str::trim).filter(|value| !value.is_empty()) else {
        return "Unknown channel".to_string();
    };

    let Ok(mut oembed_url) = reqwest::Url::parse("https://www.youtube.com/oembed") else {
        return "Unknown channel".to_string();
    };
    oembed_url
        .query_pairs_mut()
        .append_pair("url", video_url)
        .append_pair("format", "json");

    client
        .get(oembed_url)
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.json::<serde_json::Value>())
        .ok()
        .and_then(|json| {
            json.get("author_name")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "Unknown channel".to_string())
}

fn post_discord_message(
    client: &Client,
    webhook_url: &reqwest::Url,
    content: &str,
) -> Result<(), String> {
    if char_len(content) > DISCORD_MAX_MESSAGE_LENGTH {
        return Err("Internal error: Discord message exceeded 2000 characters.".to_string());
    }

    let response = client
        .post(webhook_url.clone())
        .json(&serde_json::json!({
            "content": content,
            "allowed_mentions": {
                "parse": []
            }
        }))
        .send()
        .map_err(|err| format!("Discord webhook request failed: {err}"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(format!("Discord webhook failed: HTTP {status} - {body}"));
    }

    Ok(())
}

fn post_summary_to_discord(
    client: &Client,
    webhook_url: &reqwest::Url,
    entry: &SummaryEntry,
) -> Result<(), String> {
    let summary = select_summary_for_discord(entry)?;
    let channel_name = option_trimmed(&entry.channel)
        .map(str::to_string)
        .unwrap_or_else(|| fetch_channel_name(client, option_trimmed(&entry.url)));
    let title = option_trimmed(&entry.video_name).unwrap_or("Untitled");
    let video_url = option_trimmed(&entry.url).unwrap_or_default();

    let title_line = limit_chars(&format!("**{title}**"), 500);
    let channel_text_line = limit_chars(&channel_name, 300);
    let video_line = limit_chars(&format!("Video: {video_url}"), 700);
    let summary_text = format_summary_for_discord(summary);
    let messages =
        build_discord_messages(&title_line, &channel_text_line, &video_line, &summary_text);

    for (index, message) in messages.iter().enumerate() {
        post_discord_message(client, webhook_url, message)?;
        if index < messages.len() - 1 {
            thread::sleep(Duration::from_millis(DISCORD_MESSAGE_DELAY_MS));
        }
    }

    Ok(())
}

fn send_summary_to_discord_inner(
    state: &AppState,
    request: SendSummaryToDiscordRequest,
) -> Result<(), String> {
    let webhook_url = parse_discord_webhook_url(&request.webhook_url)?;
    let entry = get_entry_by_id(state, request.id)?;
    let client = Client::new();
    post_summary_to_discord(&client, &webhook_url, &entry)
}

fn summarize_video_inner(
    state: &AppState,
    app: &AppHandle,
    window_label: &str,
    request: SummarizeVideoRequest,
) -> Result<SummaryEntry, String> {
    let SummarizeVideoRequest {
        url,
        use_whisper,
        model,
        master_prompt,
    } = request;
    let model = normalize_model(model);
    let mut args = vec![
        "summarize".to_string(),
        "--url".to_string(),
        url,
        "--model".to_string(),
        model,
    ];
    if !use_whisper {
        args.push("--no-whisper".to_string());
    }

    let prompt_path = if let Some(prompt) = normalize_prompt_template(master_prompt) {
        let path = state
            .app_dir
            .join(format!("tmp_prompt_{}.txt", now_millis()));
        fs::write(&path, prompt)
            .map_err(|err| format!("Failed to write temporary prompt file: {err}"))?;
        args.push("--prompt-template-file".to_string());
        args.push(path.to_string_lossy().into_owned());
        Some(path)
    } else {
        None
    };

    let result = run_backend_json_command(state, app, window_label, &args);
    if let Some(path) = prompt_path {
        let _ = fs::remove_file(path);
    }
    let info = result?;
    cleanup_artifacts(state, info.audio.as_deref(), info.transcript.as_deref());

    let db = open_connection(state)?;
    db.execute(
    "INSERT INTO summaries (timestamp, video_id, url, video_name, channel, thumbnail, audio, transcript, summary_en, summary_de, summary_jp)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    params![
      info.timestamp,
      info.video_id,
      info.url,
      info.video_name,
      info.channel,
      info.thumbnail,
      Option::<String>::None,
      Option::<String>::None,
      info.summary,
      Option::<String>::None,
      Option::<String>::None,
    ],
  )
  .map_err(|err| format!("Failed to save summary entry: {err}"))?;

    get_entry_by_id(state, db.last_insert_rowid())
}

fn translate_summary_inner(
    state: &AppState,
    request: TranslateSummaryRequest,
) -> Result<SummaryEntry, String> {
    let TranslateSummaryRequest {
        id,
        lang,
        model,
        prompt_template,
    } = request;
    let db = open_connection(state)?;
    let summary_text = db
        .query_row(
            "SELECT summary_en FROM summaries WHERE id = ?",
            [id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|err| format!("Failed to load English summary for translation: {err}"))?
        .flatten()
        .ok_or_else(|| "No English summary found for translation.".to_string())?;

    let tmp_summary_path = state
        .app_dir
        .join(format!("tmp_summary_{}_{}.txt", id, now_millis()));
    fs::write(&tmp_summary_path, summary_text)
        .map_err(|err| format!("Failed to write temporary summary file: {err}"))?;

    let model = normalize_model(model);
    let mut args = vec![
        "translate".to_string(),
        "--summary-file".to_string(),
        tmp_summary_path.to_string_lossy().into_owned(),
        "--lang".to_string(),
        lang.clone(),
        "--model".to_string(),
        model,
    ];
    let tmp_prompt_path = if let Some(prompt) = normalize_prompt_template(prompt_template) {
        let path = state.app_dir.join(format!(
            "tmp_translation_prompt_{}_{}.txt",
            id,
            now_millis()
        ));
        if let Err(err) = fs::write(&path, prompt) {
            let _ = fs::remove_file(&tmp_summary_path);
            return Err(format!(
                "Failed to write temporary translation prompt file: {err}"
            ));
        }
        args.push("--prompt-template-file".to_string());
        args.push(path.to_string_lossy().into_owned());
        Some(path)
    } else {
        None
    };
    let result = run_backend_text_command(state, &args);

    let _ = fs::remove_file(&tmp_summary_path);
    if let Some(path) = tmp_prompt_path {
        let _ = fs::remove_file(path);
    }
    let translation = result?;

    let column = match lang.as_str() {
        "de" => "summary_de",
        "jp" => "summary_jp",
        _ => return Err("Unsupported language code.".to_string()),
    };

    db.execute(
        &format!("UPDATE summaries SET {column} = ? WHERE id = ?"),
        params![translation, id],
    )
    .map_err(|err| format!("Failed to save translated summary: {err}"))?;

    get_entry_by_id(state, id)
}

#[tauri::command]
fn get_models() -> Result<Vec<String>, String> {
    let payload = Client::new()
        .get(OLLAMA_TAGS_URL)
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|err| format!("Failed to query Ollama models: {err}"))?
        .json::<OllamaTagsResponse>()
        .map_err(|err| format!("Failed to parse Ollama model list: {err}"))?;

    Ok(payload.models.into_iter().map(|model| model.name).collect())
}

#[tauri::command]
fn get_summaries(state: State<'_, AppState>) -> Result<Vec<SummaryEntry>, String> {
    let db = open_connection(&state)?;
    let mut stmt = db
        .prepare("SELECT * FROM summaries ORDER BY id DESC")
        .map_err(|err| format!("Failed to prepare summary query: {err}"))?;
    let rows = stmt
        .query_map([], StoredSummary::from_row)
        .map_err(|err| format!("Failed to read summaries: {err}"))?;

    let mut items = Vec::new();
    for row in rows {
        let entry = row
            .map_err(|err| format!("Failed to decode summary row: {err}"))?
            .into_entry(&state);
        items.push(entry);
    }

    Ok(items)
}

#[tauri::command]
async fn summarize_video(
    state: State<'_, AppState>,
    window: WebviewWindow,
    request: SummarizeVideoRequest,
) -> Result<SummaryEntry, String> {
    let state = state.inner().clone();
    let app = window.app_handle().clone();
    let window_label = window.label().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        summarize_video_inner(&state, &app, &window_label, request)
    })
    .await
    .map_err(|err| format!("Summarize task failed: {err}"))?
}

#[tauri::command]
fn delete_summary(state: State<'_, AppState>, request: DeleteSummaryRequest) -> Result<(), String> {
    let db = open_connection(&state)?;
    db.execute("DELETE FROM summaries WHERE id = ?", [request.id])
        .map_err(|err| format!("Failed to delete summary entry: {err}"))?;
    Ok(())
}

#[tauri::command]
async fn translate_summary(
    state: State<'_, AppState>,
    request: TranslateSummaryRequest,
) -> Result<SummaryEntry, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || translate_summary_inner(&state, request))
        .await
        .map_err(|err| format!("Translate task failed: {err}"))?
}

#[tauri::command]
async fn send_summary_to_discord(
    state: State<'_, AppState>,
    request: SendSummaryToDiscordRequest,
) -> Result<(), String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || send_summary_to_discord_inner(&state, request))
        .await
        .map_err(|err| format!("Discord task failed: {err}"))?
}

#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    that(url).map_err(|err| format!("Failed to open URL: {err}"))
}

#[tauri::command]
fn open_file(file_path: String) -> Result<(), String> {
    let path = Path::new(&file_path);
    if !path.exists() {
        return Err("Requested file does not exist.".to_string());
    }
    that(path).map_err(|err| format!("Failed to open file: {err}"))
}

fn install_app_menu(app: &mut tauri::App) -> tauri::Result<()> {
    let handle = app.handle();
    let menu_builder = MenuBuilder::new(handle);
    #[cfg(target_os = "macos")]
    let menu_builder = {
        let app_menu = SubmenuBuilder::new(handle, "YouTube Summarizer")
            .hide()
            .hide_others()
            .show_all()
            .separator()
            .quit()
            .build()?;
        menu_builder.item(&app_menu)
    };
    let settings_menu = SubmenuBuilder::new(handle, "Settings")
        .text("open_settings", "Settings...")
        .build()?;
    let edit_menu = SubmenuBuilder::new(handle, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let menu = menu_builder.item(&settings_menu).item(&edit_menu).build()?;
    app.set_menu(menu)?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .on_menu_event(|app, event| {
            if event.id() == "open_settings" {
                let _ = app.emit_to("main", "open-settings", ());
            }
        })
        .setup(|app| {
            install_app_menu(app)?;
            match ensure_app_state(app.handle()) {
                Ok(state) => {
                    app.manage(state);
                    Ok(())
                }
                Err(err) => {
                    write_startup_error_log(app.handle(), &err);
                    Err(err.into())
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_models,
            get_summaries,
            summarize_video,
            delete_summary,
            translate_summary,
            send_summary_to_discord,
            open_external,
            open_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
