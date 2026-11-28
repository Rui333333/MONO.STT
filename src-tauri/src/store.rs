//! SQLite persistence and app-owned media lifecycle.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tauri::{Manager, State};

pub struct Store {
    pub(crate) conn: Mutex<Connection>,
    data_dir: PathBuf,
    legacy_dir: PathBuf,
}

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS records (
    id            TEXT PRIMARY KEY,
    title         TEXT NOT NULL DEFAULT '',
    source_name   TEXT NOT NULL DEFAULT '',
    file_size     INTEGER NOT NULL DEFAULT 0,
    duration      REAL NOT NULL DEFAULT 0,
    sample_rate   INTEGER NOT NULL DEFAULT 16000,
    created_at    TEXT NOT NULL DEFAULT '',
    model_id      TEXT NOT NULL DEFAULT '',
    model_name    TEXT NOT NULL DEFAULT '',
    language      TEXT NOT NULL DEFAULT '',
    starred       INTEGER NOT NULL DEFAULT 0,
    audio_path    TEXT,
    source_path   TEXT,
    waveform      TEXT NOT NULL DEFAULT '[]',
    updated_at    TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_records_created ON records(created_at DESC);
CREATE TABLE IF NOT EXISTS segments (
    record_id  TEXT NOT NULL,
    position   INTEGER NOT NULL,
    start_sec  REAL NOT NULL,
    end_sec    REAL NOT NULL,
    text       TEXT NOT NULL,
    PRIMARY KEY (record_id, position),
    FOREIGN KEY (record_id) REFERENCES records(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS draft_segments (
    job_id    TEXT NOT NULL,
    attempt   INTEGER NOT NULL,
    position  INTEGER NOT NULL,
    start_sec REAL NOT NULL,
    end_sec   REAL NOT NULL,
    text      TEXT NOT NULL,
    PRIMARY KEY (job_id, attempt, position)
);
CREATE TABLE IF NOT EXISTS draft_media (
    job_id     TEXT NOT NULL,
    attempt    INTEGER NOT NULL,
    audio_path TEXT NOT NULL,
    payload    TEXT NOT NULL,
    PRIMARY KEY (job_id, attempt)
);
CREATE TABLE IF NOT EXISTS record_payload (
    id TEXT PRIMARY KEY REFERENCES records(id) ON DELETE CASCADE,
    payload TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS pending_audio_delete (path TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS kv (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

impl Store {
    pub fn open(app: &tauri::AppHandle) -> Result<Self, String> {
        let legacy_dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
        let data_dir = crate::platform::data_root()?.join("data");
        fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let database = data_dir.join("mono.db");
        let legacy_database = legacy_dir.join("mono.db");
        if !database.exists() && legacy_database.is_file() {
            let legacy = Connection::open(&legacy_database).map_err(|e| e.to_string())?;
            legacy
                .execute("VACUUM INTO ?1", [database.to_string_lossy().as_ref()])
                .map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(database).map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        let store = Self {
            conn: Mutex::new(conn),
            data_dir,
            legacy_dir,
        };
        store.relocate_existing()?;
        let conn = store.conn.lock().map_err(|e| e.to_string())?;
        store.clean_pending(&conn)?;
        drop(conn);
        Ok(store)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("mono.db")
    }

    fn owned_audio_path(&self, raw: &str) -> Option<PathBuf> {
        let path = Path::new(raw);
        if !path.is_absolute() {
            return None;
        }
        let resolved = fs::canonicalize(path).ok()?;
        let parent = resolved.parent()?;
        for root in [&self.data_dir, &self.legacy_dir] {
            for (folder, extension) in [
                ("audio", "wav"),
                ("draft-audio", "wav"),
                ("imports", "media"),
                ("recordings", "webm"),
                ("recordings", "wav"),
            ] {
                if fs::canonicalize(root.join(folder)).ok().as_deref() == Some(parent)
                    && resolved.extension()?.eq_ignore_ascii_case(extension)
                    && uuid::Uuid::parse_str(resolved.file_stem()?.to_str()?).is_ok()
                {
                    return Some(resolved);
                }
            }
        }
        None
    }

    pub fn allow_playback(&self, app: &tauri::AppHandle, path: &Path) -> Result<(), String> {
        if let Some(owned) = self.owned_audio_path(&path.to_string_lossy()) {
            if owned
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
            {
                app.asset_protocol_scope()
                    .allow_file(owned)
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    fn relocate_record(
        &self,
        tx: &rusqlite::Transaction<'_>,
        record: &mut Value,
    ) -> Result<(), String> {
        for key in ["audioPath", "sourcePath"] {
            let Some(raw) = record[key].as_str() else {
                continue;
            };
            let Some(source) = self.owned_audio_path(raw) else {
                continue;
            };
            let root = fs::canonicalize(&self.data_dir).map_err(|e| e.to_string())?;
            if source.starts_with(root) {
                continue;
            }
            let folder = self.data_dir.join(
                source
                    .parent()
                    .and_then(Path::file_name)
                    .expect("owned audio directory"),
            );
            fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
            let dest = folder.join(source.file_name().unwrap());
            if !dest.exists() {
                let part = dest.with_extension("migrating");
                fs::copy(&source, &part).map_err(|e| e.to_string())?;
                fs::rename(&part, &dest).map_err(|e| e.to_string())?;
            }
            tx.execute(
                "INSERT OR IGNORE INTO pending_audio_delete VALUES (?1)",
                [raw],
            )
            .map_err(|e| e.to_string())?;
            record[key] = json!(dest);
        }
        Ok(())
    }

    fn relocate_existing(&self) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let snapshot = read_all(&tx)?;
        for original in snapshot["records"].as_array().unwrap() {
            let mut record = original.clone();
            self.relocate_record(&tx, &mut record)?;
            if &record != original {
                write_record(&tx, &record)?;
            }
        }
        tx.commit().map_err(|e| e.to_string())
    }

    pub(super) fn clean_pending(&self, conn: &Connection) -> Result<Vec<String>, String> {
        let mut statement = conn.prepare("SELECT path FROM pending_audio_delete WHERE NOT EXISTS (SELECT 1 FROM records WHERE audio_path=path OR source_path=path)").map_err(|e| e.to_string())?;
        let paths = statement
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut pending = Vec::new();
        for raw in paths {
            if let Some(path) = self.owned_audio_path(&raw) {
                if let Err(error) = fs::remove_file(path) {
                    if error.kind() != io::ErrorKind::NotFound {
                        pending.push(raw);
                        continue;
                    }
                }
            }
            conn.execute("DELETE FROM pending_audio_delete WHERE path = ?1", [&raw])
                .map_err(|e| e.to_string())?;
        }
        Ok(pending)
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT value FROM kv WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())
    }

    pub fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO kv VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    pub fn remove(&self, key: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM kv WHERE key=?1", [key])
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

fn text(record: &Value, key: &str) -> String {
    record
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn number(record: &Value, key: &str) -> f64 {
    record.get(key).and_then(Value::as_f64).unwrap_or_default()
}

pub(crate) fn write_record(tx: &rusqlite::Transaction<'_>, record: &Value) -> Result<(), String> {
    let id = text(record, "id");
    if id.is_empty() {
        return Err("记录缺少 id".into());
    }
    let waveform =
        serde_json::to_string(&record.get("waveform").cloned().unwrap_or_else(|| json!([])))
            .map_err(|e| e.to_string())?;
    let starred = record
        .get("starred")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let audio_path = record.get("audioPath").and_then(Value::as_str);
    let source_path = record.get("sourcePath").and_then(Value::as_str);
    tx.execute(
        "INSERT INTO records (id, title, source_name, file_size, duration, sample_rate,
                              created_at, model_id, model_name, language, starred,
                              audio_path, source_path, waveform, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT(id) DO UPDATE SET
             title = excluded.title, source_name = excluded.source_name,
             file_size = excluded.file_size, duration = excluded.duration,
             sample_rate = excluded.sample_rate, created_at = excluded.created_at,
             model_id = excluded.model_id, model_name = excluded.model_name,
             language = excluded.language, starred = excluded.starred,
             audio_path = excluded.audio_path, source_path = excluded.source_path,
             waveform = excluded.waveform, updated_at = excluded.updated_at",
        params![
            id,
            text(record, "title"),
            text(record, "sourceFileName"),
            number(record, "fileSizeBytes") as i64,
            number(record, "durationSec"),
            number(record, "sampleRate") as i64,
            text(record, "createdAt"),
            text(record, "modelId"),
            text(record, "modelName"),
            text(record, "language"),
            starred as i64,
            audio_path,
            source_path,
            waveform,
            text(record, "updatedAt"),
        ],
    )
    .map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM segments WHERE record_id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    let segments = record
        .get("segments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (position, segment) in segments.iter().enumerate() {
        tx.execute(
            "INSERT INTO segments (record_id, position, start_sec, end_sec, text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                position as i64,
                number(segment, "startSec"),
                number(segment, "endSec"),
                text(segment, "text"),
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.execute("INSERT INTO record_payload VALUES (?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",
        params![id, record.to_string()]).map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn read_records(
    conn: &Connection,
    record_id: Option<&str>,
) -> Result<Vec<Value>, String> {
    let mut statement = conn
        .prepare(
            "SELECT id, title, source_name, file_size, duration, sample_rate,
                         created_at, model_id, model_name, language, starred,
                         audio_path, source_path, waveform
                  FROM records WHERE ?1 IS NULL OR id = ?1 ORDER BY created_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([record_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, String>(13)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut records = Vec::new();
    for row in rows {
        let r = row.map_err(|e| e.to_string())?;
        if let Some(payload) = conn
            .query_row(
                "SELECT payload FROM record_payload WHERE id=?1",
                [&r.0],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
        {
            records.push(serde_json::from_str(&payload).map_err(|e| e.to_string())?);
            continue;
        }
        let id = r.0.clone();
        let mut segment_stmt = conn
            .prepare(
                "SELECT start_sec, end_sec, text FROM segments
                 WHERE record_id = ?1 ORDER BY position",
            )
            .map_err(|e| e.to_string())?;
        let segments: Vec<Value> = segment_stmt
            .query_map(params![id], |row| {
                Ok((
                    row.get::<_, f64>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .enumerate()
            .map(|(index, (start, end, content))| {
                json!({
                    "id": format!("{}-{}", id, index),
                    "startSec": start,
                    "endSec": end,
                    "text": content,
                })
            })
            .collect();
        let mut record = json!({
            "id": id,
            "title": r.1,
            "sourceFileName": r.2,
            "fileSizeBytes": r.3,
            "durationSec": r.4,
            "sampleRate": r.5,
            "createdAt": r.6,
            "modelId": r.7,
            "modelName": r.8,
            "language": r.9,
            "starred": r.10 != 0,
            "waveform": serde_json::from_str::<Value>(&r.13).unwrap_or_else(|_| json!([])),
            "segments": segments,
        });
        if let Some(path) = r.11 {
            record["audioPath"] = json!(path);
        }
        if let Some(path) = r.12 {
            record["sourcePath"] = json!(path);
        }
        records.push(record);
    }
    Ok(records)
}

fn read_all(conn: &Connection) -> Result<Value, String> {
    let records = read_records(conn, None)?;
    let mut statement = conn
        .prepare("SELECT key, value FROM kv")
        .map_err(|e| e.to_string())?;
    let kv: serde_json::Map<String, Value> = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|(k, v)| (k, Value::String(v)))
        .collect();
    Ok(json!({ "records": records, "kv": kv }))
}

/// Full snapshot used by the front-end service at startup.
#[tauri::command]
pub fn store_load(app: tauri::AppHandle, state: State<'_, Store>) -> Result<Value, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let mut snapshot = read_all(&conn)?;
    for record in snapshot["records"].as_array().into_iter().flatten() {
        if let Some(path) = record["audioPath"].as_str() {
            state.allow_playback(&app, Path::new(path))?;
        }
    }
    snapshot["directory"] = json!(state.data_dir());
    Ok(snapshot)
}

/// Inspect the archived audio linked to a record.
#[tauri::command]
pub fn store_audio_status(
    state: State<'_, Store>,
    record_id: String,
) -> Result<&'static str, String> {
    let path: Option<String> = state
        .conn
        .lock()
        .map_err(|e| e.to_string())?
        .query_row(
            "SELECT audio_path FROM records WHERE id=?1
             UNION ALL SELECT audio_path FROM draft_media WHERE job_id=?1 LIMIT 1",
            [record_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();
    let Some(path) = path.filter(|path| !path.is_empty()) else {
        return Ok("unbound");
    };
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok("present"),
        Ok(_) => Ok("missing"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok("missing"),
        Err(_) => Ok("unavailable"),
    }
}

/// Write updated records. Deletion is only performed by the explicit command.
#[tauri::command]
pub fn store_save(state: State<'_, Store>, records: Vec<Value>) -> Result<(), String> {
    let mut conn = state.conn.lock().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for record in &records {
        write_record(&tx, record)?;
    }
    tx.commit().map_err(|e| e.to_string())
}

/// Explicit deletion, for callers that want to remove a single record without
/// replaying the whole snapshot. Returns the list of removed audio files.
#[tauri::command]
pub fn store_delete_records(state: State<'_, Store>, ids: Vec<String>) -> Result<Value, String> {
    delete_records(&state, ids)
}

fn delete_records(state: &Store, ids: Vec<String>) -> Result<Value, String> {
    let mut conn = state.conn.lock().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut audio_paths = Vec::new();
    for id in &ids {
        let row = tx
            .query_row(
                "SELECT audio_path, source_path FROM records WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some((audio, source)) = row {
            audio_paths.extend(audio);
            audio_paths.extend(source);
        }
        tx.execute("DELETE FROM records WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
    }
    for path in audio_paths {
        tx.execute("INSERT OR IGNORE INTO pending_audio_delete
                    SELECT ?1 WHERE NOT EXISTS (SELECT 1 FROM records WHERE audio_path=?1 OR source_path=?1)", [&path])
            .map_err(|e| e.to_string())?;
    }
    for id in ids {
        tx.execute("INSERT INTO kv VALUES (?1, 'null') ON CONFLICT(key) DO UPDATE SET value=excluded.value", [format!("feishu.binding.{id}")]).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(json!({"pendingFiles": state.clean_pending(&conn)?}))
}

#[tauri::command]
pub async fn complete_onboarding(
    app: tauri::AppHandle,
    settings: Value,
    model: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = crate::models::directory(settings["modelsDir"].as_str())?.join(&model);
        crate::models::verify_installed(&app, &path, &model)?;
        let state = app.state::<Store>();
        let mut conn = state.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for (key, value) in [
            ("inkvoice_settings_v1", settings.to_string()),
            ("inkvoice_active_model_v1", model),
            ("mono_onboarding_v1", "1".into()),
        ] {
            tx.execute(
            "INSERT INTO kv VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )
        .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn store_kv_get(state: State<'_, Store>, key: String) -> Result<Option<String>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    conn.query_row("SELECT value FROM kv WHERE key = ?1", params![key], |row| {
        row.get::<_, String>(0)
    })
    .optional()
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn store_kv_set(state: State<'_, Store>, key: String, value: String) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO kv (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn store_kv_delete(state: State<'_, Store>, key: String) -> Result<(), String> {
    state.remove(&key)
}

/// One-shot import used when migrating legacy IndexedDB / localStorage data.
/// Existing SQLite rows win: a repeated import never overwrites newer data.
#[tauri::command]
pub fn store_import(
    state: State<'_, Store>,
    records: Vec<Value>,
    kv: Option<Value>,
) -> Result<Value, String> {
    let mut conn = state.conn.lock().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut imported = 0usize;
    for record in &records {
        let id = text(record, "id");
        if id.is_empty() {
            continue;
        }
        let exists: bool = tx
            .query_row("SELECT 1 FROM records WHERE id = ?1", params![id], |_| {
                Ok(true)
            })
            .optional()
            .map_err(|e| e.to_string())?
            .unwrap_or(false);
        if exists {
            continue;
        }
        let mut record = record.clone();
        state.relocate_record(&tx, &mut record)?;
        write_record(&tx, &record)?;
        imported += 1;
    }
    if let Some(Value::Object(entries)) = kv {
        for (key, value) in entries {
            let serialized = match value {
                Value::String(s) => s,
                other => other.to_string(),
            };
            tx.execute(
                "INSERT OR IGNORE INTO kv (key, value) VALUES (?1, ?2)",
                params![key, serialized],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    state.clean_pending(&conn)?;
    Ok(json!({ "imported": imported }))
}

#[tauri::command]
pub fn store_stats(state: State<'_, Store>) -> Result<Value, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let records: i64 = conn
        .query_row("SELECT COUNT(*) FROM records", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    let segments: i64 = conn
        .query_row("SELECT COUNT(*) FROM segments", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    let keys: i64 = conn
        .query_row("SELECT COUNT(*) FROM kv", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "database": state.database_path().to_string_lossy(),
        "records": records,
        "segments": segments,
        "keys": keys,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_paged_draft_and_exports_after_reopening_then_promotes_retry() {
        use crate::drafts::{page, write_draft, Draft};

        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("mono.db");
        let open = || {
            let conn = Connection::open(&database).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            Store {
                conn: Mutex::new(conn),
                data_dir: root.path().to_owned(),
                legacy_dir: root.path().to_owned(),
            }
        };
        {
            let store = open();
            let mut draft = Draft::start(&store, "job").unwrap();
            for index in 0..231 {
                let start = index as f64;
                draft
                    .append(start, start + 0.5, &format!("第 {index} 段"))
                    .unwrap();
            }
        }
        let store = open();
        {
            let conn = store.conn.lock().unwrap();
            let latest = page(&conn, "job", None, None, None).unwrap();
            assert_eq!(latest["total"], 231);
            assert_eq!(latest["start"], 131);
            assert_eq!(latest["savedUntil"], 230.5);
            assert_eq!(latest["segments"].as_array().unwrap().len(), 100);
            assert_eq!(
                page(&conn, "job", None, None, Some((1, 131, 231))).unwrap(),
                Value::Null
            );
            let first = page(&conn, "job", Some(0), None, None).unwrap();
            assert_eq!(first["segments"][0]["text"], "第 0 段");
            let mut output = Vec::new();
            write_draft(&conn, "job", None, "srt", &mut output).unwrap();
            let output = String::from_utf8(output).unwrap();
            assert_eq!(output.matches(" --> ").count(), 231);
            assert!(output.ends_with("231\n00:03:50,000 --> 00:03:50,500\n第 230 段\n\n"));
            assert!(read_records(&conn, None).unwrap().is_empty());
        }
        let mut retry = Draft::start(&store, "job").unwrap();
        assert!(retry.finish(&json!({"id":"job"})).is_err());
        let attempt = retry.append(0.0, 1.0, "CUDA 部分稿").unwrap();
        {
            let conn = store.conn.lock().unwrap();
            assert_eq!(
                page(&conn, "job", None, Some(attempt), None).unwrap()["total"],
                1
            );
            assert_eq!(page(&conn, "job", None, None, None).unwrap()["total"], 231);
        }
        retry.reset().unwrap();
        retry.append(0.0, 2.0, "CPU 完成稿").unwrap();
        let saved = retry
            .finish(&json!({"id":"job","title":"访谈","sampleRate":16000}))
            .unwrap();
        assert_eq!(saved["segments"].as_array().unwrap().len(), 1);
        assert_eq!(saved["segments"][0]["text"], "CPU 完成稿");
        let conn = store.conn.lock().unwrap();
        assert_eq!(page(&conn, "job", None, None, None).unwrap()["total"], 0);
        assert_eq!(read_records(&conn, Some("job")).unwrap(), vec![saved]);
    }

    #[test]
    fn resumes_the_same_attempt_from_verified_audio_and_keeps_committed_rows() {
        use crate::drafts::{fingerprint, page, Draft};

        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.mp4");
        let audio = root.path().join("retained.wav");
        fs::write(&source, "source").unwrap();
        fs::write(&audio, "audio").unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let store = Store {
            conn: Mutex::new(conn),
            data_dir: root.path().to_owned(),
            legacy_dir: root.path().to_owned(),
        };
        let mut draft = Draft::start(&store, "job").unwrap();
        let settings = json!({"computeDevice":"cpu", "language":"zh"});
        draft.remember_audio(&audio, json!({
            "source":source, "sourceStamp":fingerprint(&source).unwrap(), "options":settings,
            "model":"faster-whisper-large-v3-turbo",
        }), false).unwrap();
        draft.set_language("zh").unwrap();
        let attempt = draft.append(0.0, 10.0, "good prefix").unwrap();
        let (mut resumed, checkpoint) =
            Draft::resume(&store, "job", &source, "faster-whisper-large-v3-turbo").unwrap();
        assert_eq!(checkpoint["boundary"], 10.0);
        assert_eq!(checkpoint["options"], settings);
        assert_eq!(checkpoint["language"], "zh");
        assert!(Draft::resume(&store, "job", &source, "faster-whisper-large-v3").is_err());
        resumed.reset().unwrap();
        assert_eq!(resumed.append(9.0, 9.8, "covered").unwrap(), attempt);
        assert_eq!(resumed.append(9.8, 11.0, "new words").unwrap(), attempt);
        {
            let conn = store.conn.lock().unwrap();
            let result = page(&conn, "job", None, None, None).unwrap();
            assert_eq!(result["total"], 2);
            assert_eq!(result["segments"][0]["text"], "good prefix");
            assert_eq!(result["segments"][1]["startSec"], 10.0);
        }
        fs::write(&source, "modified source").unwrap();
        assert!(Draft::resume(&store, "job", &source, "faster-whisper-large-v3-turbo").is_err());
        fs::remove_file(&source).unwrap();
        assert!(Draft::resume(&store, "job", &source, "faster-whisper-large-v3-turbo").is_ok());
        fs::remove_file(&audio).unwrap();
        assert!(Draft::resume(&store, "job", &source, "faster-whisper-large-v3-turbo").is_err());
    }

    #[test]
    fn migrates_and_deletes_owned_media_without_touching_imported_source() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let data_dir = root.join("data");
        let legacy_dir = root.join("legacy");
        fs::create_dir_all(legacy_dir.join("audio")).unwrap();
        fs::create_dir_all(legacy_dir.join("recordings")).unwrap();
        fs::create_dir_all(data_dir.join("recordings")).unwrap();
        let id = uuid::Uuid::new_v4();
        let legacy_audio = legacy_dir.join("audio").join(format!("{id}.wav"));
        let imported = root.join(format!("{id}.wav"));
        let recording = data_dir.join("recordings").join(format!("{id}.webm"));
        let legacy_recording = legacy_dir.join("recordings").join(format!("{id}.wav"));
        for path in [&legacy_audio, &imported, &recording, &legacy_recording] {
            fs::write(path, b"test media").unwrap();
        }

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let store = Store {
            conn: Mutex::new(conn),
            data_dir,
            legacy_dir,
        };
        {
            let mut conn = store.conn.lock().unwrap();
            let tx = conn.transaction().unwrap();
            for (id, source) in [
                ("a", &imported),
                ("b", &recording),
                ("c", &legacy_recording),
            ] {
                let mut record = json!({"id":id,"audioPath":legacy_audio,"sourcePath":source,
                    "notes":"keep these notes","segments":[{"id":"seg","text":"test","speaker":"A"}]});
                store.relocate_record(&tx, &mut record).unwrap();
                write_record(&tx, &record).unwrap();
            }
            tx.commit().unwrap();
            assert!(store.clean_pending(&conn).unwrap().is_empty());
            let snapshot = read_all(&conn).unwrap();
            assert_eq!(snapshot["records"][0]["notes"], "keep these notes");
            assert_eq!(snapshot["records"][0]["segments"][0]["speaker"], "A");
        }
        let relocated = store.data_dir.join("audio").join(format!("{id}.wav"));
        let relocated_recording = store.data_dir.join("recordings").join(format!("{id}.wav"));
        assert!(!legacy_audio.exists() && relocated.exists());
        assert!(!legacy_recording.exists() && relocated_recording.exists());
        assert_eq!(
            delete_records(&store, vec!["a".into()]).unwrap()["pendingFiles"],
            json!([])
        );
        assert!(relocated.exists() && imported.exists());
        assert_eq!(
            delete_records(&store, vec!["b".into()]).unwrap()["pendingFiles"],
            json!([])
        );
        assert!(relocated.exists() && !recording.exists() && imported.exists());
        assert_eq!(
            delete_records(&store, vec!["c".into()]).unwrap()["pendingFiles"],
            json!([])
        );
        assert!(!relocated.exists() && !relocated_recording.exists() && imported.exists());
        assert_eq!(
            read_all(&store.conn.lock().unwrap()).unwrap()["records"],
            json!([])
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
