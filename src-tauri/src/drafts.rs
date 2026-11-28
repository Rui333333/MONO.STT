use crate::store::{self, Store};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, BufWriter, Read, Seek, Write},
    path::Path,
};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

const PAGE_SIZE: i64 = 100;

pub fn fingerprint(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("请选择一个本地媒体文件".into());
    }
    let modified = metadata.modified().map_err(|e| e.to_string())?;
    Ok(format!("{}:{modified:?}", metadata.len()))
}

fn audio_digest(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn read_media(conn: &Connection, id: &str, attempt: i64) -> Result<Option<Value>, String> {
    let payload: Option<String> = conn
        .query_row(
            "SELECT payload FROM draft_media WHERE job_id = ?1 AND (?2 = 0 OR attempt = ?2)
         ORDER BY attempt DESC LIMIT 1",
            params![id, attempt],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    payload
        .map(|value| serde_json::from_str(&value).map_err(|e| e.to_string()))
        .transpose()
}

fn audio_path(media: &Value) -> Option<&Path> {
    media["audioPath"].as_str().map(Path::new)
}

fn available_audio(media: &Value) -> bool {
    audio_path(media)
        .and_then(|path| fingerprint(path).ok())
        .as_deref()
        == media["audioStamp"].as_str()
        && media["audioStamp"].is_string()
}

fn resume_error(media: &Value, source: Option<&Path>) -> Option<&'static str> {
    if !available_audio(media) {
        return Some("已保存的音轨缺失或已变化，请从头转录");
    }
    if let Some(source) = source {
        if Some(source) != media["source"].as_str().map(Path::new) {
            return Some("导入来源发生变化，请从头转录");
        }
        if source.exists() && fingerprint(source).ok().as_deref() != media["sourceStamp"].as_str() {
            return Some("原文件已被修改，请从头转录");
        }
    }
    None
}

fn clear_media(tx: &Transaction<'_>, id: &str, kept: Option<&str>) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO pending_audio_delete
         SELECT audio_path FROM draft_media WHERE job_id = ?1 AND audio_path <> COALESCE(?2, '')",
        params![id, kept],
    )?;
    tx.execute("DELETE FROM draft_media WHERE job_id = ?1", [id])?;
    Ok(())
}

pub fn clean_orphans(store: &Store) -> Result<(), String> {
    let folder = store.data_dir().join("draft-audio");
    if !folder.is_dir() {
        return Ok(());
    }
    let conn = store.conn.lock().map_err(|e| e.to_string())?;
    for entry in fs::read_dir(folder).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        if !matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("wav" | "transcribing")
        ) || !path
            .file_stem()
            .and_then(|value| value.to_str())
            .is_some_and(|value| uuid::Uuid::parse_str(value).is_ok())
        {
            continue;
        }
        let referenced: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM draft_media WHERE audio_path = ?1
                           UNION ALL SELECT 1 FROM records WHERE audio_path = ?1)",
                [path.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !referenced {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

pub struct Draft<'a> {
    store: &'a Store,
    id: &'a str,
    attempt: i64,
    position: i64,
    resumed: bool,
    boundary: f64,
    last_end: f64,
}

impl<'a> Draft<'a> {
    pub fn start(store: &'a Store, id: &'a str) -> Result<Self, String> {
        let conn = store.conn.lock().map_err(|e| e.to_string())?;
        let attempt: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(attempt), 0) + 1 FROM (
                    SELECT MAX(attempt) AS attempt FROM draft_segments WHERE job_id = ?1
                    UNION ALL SELECT MAX(attempt) FROM draft_media WHERE job_id = ?1)",
                [id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let best = pick_attempt(&conn, id, None).map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM draft_segments WHERE job_id = ?1 AND attempt <> ?2",
            params![id, best],
        )
        .map_err(|e| e.to_string())?;
        Ok(Self {
            store,
            id,
            attempt,
            position: 0,
            resumed: false,
            boundary: 0.0,
            last_end: 0.0,
        })
    }

    pub fn resume(
        store: &'a Store,
        id: &'a str,
        source: &Path,
        model: &str,
    ) -> Result<(Self, Value), String> {
        let conn = store.conn.lock().map_err(|e| e.to_string())?;
        let attempt = pick_attempt(&conn, id, None).map_err(|e| e.to_string())?;
        if attempt == 0 {
            return Err("没有可续转的已保存段落，请从头转录".into());
        }
        let media = read_media(&conn, id, attempt)?.ok_or("此任务缺少续转检查点，请从头转录")?;
        if let Some(reason) = resume_error(&media, Some(source)) {
            return Err(reason.into());
        }
        if media["model"].as_str() != Some(model) {
            return Err("模型已更换，请从头转录".into());
        }
        let mut statement = conn.prepare(
            "SELECT position, end_sec, text FROM draft_segments WHERE job_id = ?1 AND attempt = ?2
             ORDER BY position DESC LIMIT 8",
        ).map_err(|e| e.to_string())?;
        let tail = statement
            .query_map(params![id, attempt], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        let (position, boundary, _) = tail.first().ok_or("没有可续转的已保存段落")?;
        let (position, boundary) = (*position + 1, *boundary);
        let previous = tail
            .iter()
            .rev()
            .map(|(_, _, text)| text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        drop(statement);
        drop(conn);
        let cached = audio_path(&media).ok_or("缺少已保存音轨")?;
        if Some(audio_digest(cached)?.as_str()) != media["audioSha256"].as_str() {
            return Err("已保存音轨完整性校验失败，请从头转录".into());
        }
        let mut context = media;
        context["boundary"] = json!(boundary);
        context["previous"] = json!(previous);
        Ok((
            Self {
                store,
                id,
                attempt,
                position,
                resumed: true,
                boundary,
                last_end: boundary,
            },
            context,
        ))
    }

    pub fn cached_audio(&self, source: &Path) -> Result<Option<Value>, String> {
        let conn = self.store.conn.lock().map_err(|e| e.to_string())?;
        let best = pick_attempt(&conn, self.id, None).map_err(|e| e.to_string())?;
        let media = read_media(&conn, self.id, best)?;
        drop(conn);
        if let Some(media) = media {
            if resume_error(&media, Some(source)).is_none() {
                let cached = audio_path(&media).ok_or("缺少已保存音轨")?;
                if Some(audio_digest(cached)?.as_str()) == media["audioSha256"].as_str() {
                    return Ok(Some(media));
                }
            }
        }
        Ok(None)
    }

    pub fn remember_audio(
        &self,
        path: &Path,
        mut media: Value,
        reused: bool,
    ) -> Result<(), String> {
        if self.resumed {
            return Ok(());
        }
        if !reused {
            media["audioPath"] = json!(path);
            let stamp = fingerprint(path)?;
            media["audioSha256"] = json!(audio_digest(path)?);
            if fingerprint(path)? != stamp {
                return Err("提取后的音轨发生变化，请重试".into());
            }
            media["audioStamp"] = json!(stamp);
        }
        let conn = self.store.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO draft_media (job_id, attempt, audio_path, payload) VALUES (?1, ?2, ?3, ?4)",
            params![self.id, self.attempt, path.to_string_lossy(), media.to_string()],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn set_language(&self, language: &str) -> Result<(), String> {
        if language.is_empty() {
            return Ok(());
        }
        let conn = self.store.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE draft_media SET payload = json_set(payload, '$.language', ?3)
                      WHERE job_id = ?1 AND attempt = ?2",
            params![self.id, self.attempt, language],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn reset(&mut self) -> Result<(), String> {
        if !self.resumed && self.position > 0 {
            let conn = self.store.conn.lock().map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO draft_media (job_id, attempt, audio_path, payload)
                 SELECT job_id, ?3, audio_path, payload FROM draft_media WHERE job_id = ?1 AND attempt = ?2",
                params![self.id, self.attempt, self.attempt + 1],
            ).map_err(|e| e.to_string())?;
            self.attempt += 1;
            self.position = 0;
            self.last_end = 0.0;
        }
        Ok(())
    }

    pub fn append(&mut self, start: f64, end: f64, text: &str) -> Result<i64, String> {
        let start = if self.resumed {
            start.max(self.last_end).max(self.boundary)
        } else {
            start
        };
        if end <= start {
            return Ok(self.attempt);
        }
        let conn = self.store.conn.lock().map_err(|e| e.to_string())?;
        conn.prepare_cached("INSERT INTO draft_segments VALUES (?1, ?2, ?3, ?4, ?5, ?6)")
            .and_then(|mut statement| {
                statement.execute(params![
                    self.id,
                    self.attempt,
                    self.position,
                    start,
                    end,
                    text
                ])
            })
            .map_err(|e| e.to_string())?;
        self.position += 1;
        self.last_end = end;
        Ok(self.attempt)
    }

    pub fn finish(&self, record: &Value) -> Result<Value, String> {
        let mut conn = self.store.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        if self.position == 0 && pick_attempt(&tx, self.id, None).map_err(|e| e.to_string())? != 0 {
            return Err("重新转录未识别到文字，已保留上次的部分稿".into());
        }
        store::write_record(&tx, record)?;
        tx.execute("DELETE FROM record_payload WHERE id = ?1", [self.id])
            .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO segments (record_id, position, start_sec, end_sec, text)
             SELECT job_id, position, start_sec, end_sec, text
             FROM draft_segments WHERE job_id = ?1 AND attempt = ?2",
            params![self.id, self.attempt],
        )
        .map_err(|e| e.to_string())?;
        clear_media(&tx, self.id, record["audioPath"].as_str()).map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM draft_segments WHERE job_id = ?1", [self.id])
            .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR IGNORE INTO kv VALUES ('libraryCreated', '1')",
            [],
        )
        .map_err(|e| e.to_string())?;
        let saved = store::read_records(&tx, Some(self.id))?
            .pop()
            .ok_or("转录记录保存失败")?;
        tx.commit().map_err(|e| e.to_string())?;
        let _ = self.store.clean_pending(&conn);
        Ok(saved)
    }
}

fn pick_attempt(conn: &Connection, id: &str, preferred: Option<i64>) -> rusqlite::Result<i64> {
    if let Some(attempt) = preferred {
        let exists = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM draft_segments WHERE job_id = ?1 AND attempt = ?2)",
            params![id, attempt],
            |row| row.get(0),
        )?;
        if exists {
            return Ok(attempt);
        }
    }
    conn.query_row(
        "SELECT COALESCE((SELECT attempt FROM draft_segments WHERE job_id = ?1
         GROUP BY attempt ORDER BY MAX(end_sec) DESC, attempt DESC LIMIT 1), 0)",
        [id],
        |row| row.get(0),
    )
}

pub(super) fn page(
    conn: &Connection,
    id: &str,
    requested: Option<i64>,
    preferred: Option<i64>,
    known: Option<(i64, i64, i64)>,
) -> rusqlite::Result<Value> {
    let attempt = pick_attempt(conn, id, preferred)?;
    let (total, saved_until): (i64, f64) = conn
        .query_row(
            "SELECT position + 1, end_sec FROM draft_segments
         WHERE job_id = ?1 AND attempt = ?2 ORDER BY position DESC LIMIT 1",
            params![id, attempt],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .unwrap_or_default();
    let latest = (total - PAGE_SIZE).max(0);
    let start = requested.unwrap_or(latest).clamp(0, latest);
    if known == Some((attempt, start, total)) {
        return Ok(Value::Null);
    }
    let mut statement = conn.prepare_cached(
        "SELECT position, start_sec, end_sec, text FROM draft_segments
         WHERE job_id = ?1 AND attempt = ?2 AND position >= ?3 ORDER BY position LIMIT ?4",
    )?;
    let segments = statement
        .query_map(params![id, attempt, start, PAGE_SIZE], |row| {
            let position: i64 = row.get(0)?;
            Ok(
                json!({"id":format!("{id}-{attempt}-{position}"), "startSec":row.get::<_, f64>(1)?,
                  "endSec":row.get::<_, f64>(2)?, "text":row.get::<_, String>(3)?}),
            )
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(
        json!({"attempt":attempt, "start":start, "total":total, "savedUntil":saved_until, "segments":segments}),
    )
}

#[tauri::command]
pub fn draft_page(
    app: tauri::AppHandle,
    state: State<'_, Store>,
    job_id: String,
    start: Option<i64>,
    attempt: Option<i64>,
    known: Option<(i64, i64, i64)>,
) -> Result<Value, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let mut output = page(
        &conn,
        &job_id,
        start,
        attempt,
        known.filter(|(_, _, total)| *total > 0),
    )
    .map_err(|e| e.to_string())?;
    if output.is_null() {
        return Ok(output);
    }
    let current = output["attempt"].as_i64().unwrap_or_default();
    let media = read_media(&conn, &job_id, current)?;
    drop(conn);
    if let Some(media) = media {
        let reason = resume_error(&media, media["source"].as_str().map(Path::new));
        if let Some(path) = audio_path(&media).filter(|_| available_audio(&media)) {
            state.allow_playback(&app, path)?;
            output["audioPath"] = json!(path);
            output["durationSec"] = media["duration"].clone();
            output["waveform"] = media["waveform"].clone();
        }
        output["canResume"] = json!(current > 0 && reason.is_none());
        output["resumeReason"] = json!(reason);
    } else if current > 0 {
        output["resumeReason"] = json!("此任务缺少续转检查点，请从头转录");
    }
    Ok(output)
}

#[tauri::command]
pub fn draft_discard(state: State<'_, Store>, job_id: String) -> Result<(), String> {
    let mut conn = state.conn.lock().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM draft_segments WHERE job_id = ?1", [&job_id])
        .map_err(|e| e.to_string())?;
    clear_media(&tx, &job_id, None).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    let _ = state.clean_pending(&conn)?;
    Ok(())
}

fn timestamp(seconds: f64) -> String {
    let ms = (seconds * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        ms % 1000
    )
}

pub(super) fn write_draft(
    conn: &Connection,
    id: &str,
    preferred: Option<i64>,
    format: &str,
    output: &mut impl Write,
) -> Result<(), String> {
    let attempt = pick_attempt(conn, id, preferred).map_err(|e| e.to_string())?;
    let mut statement = conn
        .prepare(
            "SELECT start_sec, end_sec, text FROM draft_segments
         WHERE job_id = ?1 AND attempt = ?2 ORDER BY position",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = statement
        .query(params![id, attempt])
        .map_err(|e| e.to_string())?;
    let mut index = 0;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        index += 1;
        let text: String = row.get(2).map_err(|e| e.to_string())?;
        if format == "srt" {
            let start = timestamp(row.get(0).map_err(|e| e.to_string())?);
            let end = timestamp(row.get(1).map_err(|e| e.to_string())?);
            writeln!(
                output,
                "{index}\n{start} --> {end}\n{}\n",
                text.lines().collect::<Vec<_>>().join(" ")
            )
        } else {
            if index == 1 {
                writeln!(output, "【部分转录稿】\n").map_err(|e| e.to_string())?;
            }
            writeln!(output, "{text}")
        }
        .map_err(|e| e.to_string())?;
    }
    if index == 0 {
        return Err("没有可导出的已保存段落".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn draft_export(
    app: tauri::AppHandle,
    job_id: String,
    attempt: Option<i64>,
    title: String,
    format: String,
) -> Result<bool, String> {
    if !matches!(format.as_str(), "txt" | "srt") {
        return Err("不支持的导出格式".into());
    }
    let db = app.state::<Store>().database_path();
    let (mut snapshot, format) = tauri::async_runtime::spawn_blocking(move || {
        let mut connection = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
        let tx = connection.transaction().map_err(|e| e.to_string())?;
        let mut snapshot = tempfile::tempfile().map_err(|e| e.to_string())?;
        {
            let mut output = BufWriter::new(&mut snapshot);
            write_draft(&tx, &job_id, attempt, &format, &mut output)?;
            output.flush().map_err(|e| e.to_string())?;
        }
        snapshot.rewind().map_err(|e| e.to_string())?;
        Ok::<_, String>((snapshot, format))
    })
    .await
    .map_err(|e| e.to_string())??;
    let name = format!(
        "{}（部分稿）.{format}",
        crate::batch_export::filename_stem(&title)
    );
    let Some(file) = app
        .dialog()
        .file()
        .add_filter(format.to_uppercase(), &[&format])
        .set_file_name(name)
        .blocking_save_file()
    else {
        return Ok(false);
    };
    let path = file.into_path().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let directory = path.parent().unwrap_or(Path::new("."));
        let mut temporary =
            tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
        io::copy(&mut snapshot, temporary.as_file_mut()).map_err(|e| e.to_string())?;
        temporary.persist(path).map_err(|e| e.to_string())?;
        Ok(true)
    })
    .await
    .map_err(|e| e.to_string())?
}
