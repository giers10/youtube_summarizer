#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    env, fs,
    io::{BufRead, BufReader, ErrorKind},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use open::that;
use reqwest::blocking::Client;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

const DEFAULT_MODEL: &str = "mistral:latest";
const OLLAMA_TAGS_URL: &str = "http://localhost:11434/api/tags";
const BACKEND_EXECUTABLE_NAME: &str = "yts-backend";
const TARGET_TRIPLE: &str = env!("TAURI_BUILD_TARGET");

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
}

#[derive(Debug, Deserialize)]
struct DeleteSummaryRequest {
    id: i64,
}

#[derive(Debug, Deserialize)]
struct TranslateSummaryRequest {
    id: i64,
    lang: String,
    model: Option<String>,
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

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
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

    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join(relative_path));
    }

    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join(relative_path),
    );

    candidates.into_iter().find(|path| path.exists())
}

fn resolve_backend_binary(app: &AppHandle) -> Option<PathBuf> {
    if let Ok(path) = env::var("YTS_BACKEND_BIN") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    let relative_path = Path::new("backend")
        .join(TARGET_TRIPLE)
        .join(platform_executable_name(BACKEND_EXECUTABLE_NAME));
    resolve_resource_file(app, &relative_path)
}

fn resolve_script_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Ok(resource_dir) = app.path().resource_dir() {
        if resource_dir.join("backend_cli.py").exists() {
            return Ok(resource_dir);
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
    if let Some(executable) = resolve_backend_binary(app) {
        return Ok(BackendRuntime::Bundled { executable });
    }

    let script_dir = resolve_script_dir(app)?;
    let python = resolve_python_command(&script_dir)?;
    Ok(BackendRuntime::Python { python, script_dir })
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
    .prepare("SELECT id, audio, transcript FROM summaries WHERE audio IS NOT NULL OR transcript IS NOT NULL")
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

fn apply_backend_env(command: &mut Command, state: &AppState) {
    command.env("PYTHONUNBUFFERED", "1");
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

    let mut child = build_backend_command(state, &command_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Failed to start bundled backend: {err}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Backend stdout was not captured.".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Backend stderr was not captured.".to_string())?;
    let stderr_buffer = Arc::new(Mutex::new(String::new()));

    let stdout_app = app.clone();
    let stdout_label = window_label.to_string();
    let stdout_handle = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => emit_progress(&stdout_app, &stdout_label, &line),
                Err(_) => break,
            }
        }
    });

    let stderr_app = app.clone();
    let stderr_label = window_label.to_string();
    let stderr_buffer_clone = Arc::clone(&stderr_buffer);
    let stderr_handle = thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            match line {
                Ok(line) => {
                    emit_progress(&stderr_app, &stderr_label, &line);
                    if let Ok(mut buffer) = stderr_buffer_clone.lock() {
                        buffer.push_str(&line);
                        buffer.push('\n');
                    }
                }
                Err(_) => break,
            }
        }
    });

    let status = child
        .wait()
        .map_err(|err| format!("Failed to wait for bundled backend: {err}"))?;

    let _ = stdout_handle.join();
    let _ = stderr_handle.join();

    if !status.success() {
        let stderr_output = stderr_buffer
            .lock()
            .map(|buffer| buffer.trim().to_string())
            .unwrap_or_else(|_| String::new());
        let message = if stderr_output.is_empty() {
            format!("Bundled backend exited with status {status}.")
        } else {
            stderr_output
        };
        let _ = fs::remove_file(&output_path);
        return Err(message);
    }

    let raw_json = fs::read_to_string(&output_path)
        .map_err(|err| format!("Failed to read backend output JSON: {err}"))?;
    let _ = fs::remove_file(&output_path);

    serde_json::from_str(&raw_json).map_err(|err| format!("Invalid backend output JSON: {err}"))
}

fn run_backend_text_command(state: &AppState, args: &[String]) -> Result<String, String> {
    let output = build_backend_command(state, args)
        .output()
        .map_err(|err| format!("Failed to start translation backend: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("Translation backend exited with status {}.", output.status)
        } else {
            stderr
        });
    }

    let translation = String::from_utf8(output.stdout)
        .map_err(|err| format!("Translation backend returned invalid UTF-8: {err}"))?
        .trim()
        .to_string();
    if translation.is_empty() {
        return Err("Translation backend returned an empty result.".to_string());
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

fn summarize_video_inner(
    state: &AppState,
    app: &AppHandle,
    window_label: &str,
    request: SummarizeVideoRequest,
) -> Result<SummaryEntry, String> {
    let model = normalize_model(request.model);
    let mut args = vec![
        "summarize".to_string(),
        "--url".to_string(),
        request.url,
        "--model".to_string(),
        model,
    ];
    if !request.use_whisper {
        args.push("--no-whisper".to_string());
    }

    let info = run_backend_json_command(state, app, window_label, &args)?;
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
    let db = open_connection(state)?;
    let summary_text = db
        .query_row(
            "SELECT summary_en FROM summaries WHERE id = ?",
            [request.id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|err| format!("Failed to load English summary for translation: {err}"))?
        .flatten()
        .ok_or_else(|| "No English summary found for translation.".to_string())?;

    let tmp_summary_path =
        state
            .app_dir
            .join(format!("tmp_summary_{}_{}.txt", request.id, now_millis()));
    fs::write(&tmp_summary_path, summary_text)
        .map_err(|err| format!("Failed to write temporary summary file: {err}"))?;

    let model = normalize_model(request.model);
    let args = vec![
        "translate".to_string(),
        "--summary-file".to_string(),
        tmp_summary_path.to_string_lossy().into_owned(),
        "--lang".to_string(),
        request.lang.clone(),
        "--model".to_string(),
        model,
    ];
    let result = run_backend_text_command(state, &args);

    let _ = fs::remove_file(&tmp_summary_path);
    let translation = result?;

    let column = match request.lang.as_str() {
        "de" => "summary_de",
        "jp" => "summary_jp",
        _ => return Err("Unsupported language code.".to_string()),
    };

    db.execute(
        &format!("UPDATE summaries SET {column} = ? WHERE id = ?"),
        params![translation, request.id],
    )
    .map_err(|err| format!("Failed to save translated summary: {err}"))?;

    get_entry_by_id(state, request.id)
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

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let state = ensure_app_state(app.handle())?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_models,
            get_summaries,
            summarize_video,
            delete_summary,
            translate_summary,
            open_external,
            open_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
