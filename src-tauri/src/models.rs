use std::{
    collections::HashMap,
    fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{Emitter, State};
use tauri_plugin_dialog::DialogExt;

const FILES: [&str; 5] = [
    "config.json",
    "tokenizer.json",
    "vocabulary.json",
    "preprocessor_config.json",
    "model.bin",
];
const MODELS: [(&str, &str); 2] = [
    (
        "faster-whisper-large-v3-turbo",
        "dropbox-dash/faster-whisper-large-v3-turbo",
    ),
    ("faster-whisper-large-v3", "Systran/faster-whisper-large-v3"),
];

type Downloads = HashMap<(PathBuf, String), Arc<AtomicBool>>;
#[derive(Default)]
pub struct ModelDownloads(Mutex<Downloads>);

pub fn directory(configured: Option<&str>) -> Result<PathBuf, String> {
    if let Some(value) = configured.map(str::trim).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err("请选择一个绝对路径作为模型目录".into());
        }
        return Ok(path);
    }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    Ok(executable.parent().ok_or("找不到应用目录")?.join("models"))
}

pub fn installed(path: &Path) -> bool {
    FILES.into_iter().all(|file| {
        fs::metadata(path.join(file)).is_ok_and(|entry| entry.is_file() && entry.len() > 0)
    })
}

pub fn valid_model(model: &str) -> bool {
    MODELS.iter().any(|(id, _)| *id == model)
}

#[tauri::command]
pub fn default_models_dir() -> Result<PathBuf, String> {
    directory(None)
}

#[tauri::command]
pub async fn pick_models_dir(app: tauri::AppHandle, models_dir: String) -> Option<PathBuf> {
    let mut dialog = app.dialog().file();
    if let Ok(path) = directory(Some(&models_dir)) {
        if path.is_dir() {
            dialog = dialog.set_directory(path);
        }
    }
    dialog.blocking_pick_folder()?.into_path().ok()
}

#[tauri::command]
pub fn list_local_models(models_dir: String) -> Result<Vec<&'static str>, String> {
    let root = directory(Some(&models_dir))?;
    if root.exists() {
        fs::read_dir(&root).map_err(|error| format!("无法读取模型目录：{error}"))?;
    }
    Ok(MODELS
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| installed(&root.join(id)))
        .collect())
}

#[tauri::command]
pub async fn download_model(
    app: tauri::AppHandle,
    downloads: State<'_, ModelDownloads>,
    models_dir: String,
    model: String,
    source: String,
    request_id: String,
) -> Result<bool, String> {
    let task = crate::updates::task(&app)?;
    let (_, repo) = MODELS
        .iter()
        .find(|(id, _)| *id == model)
        .ok_or("未知模型")?;
    let host = match source.as_str() {
        "huggingface" => "https://huggingface.co",
        "hf-mirror" => "https://hf-mirror.com",
        _ => return Err("未知下载源".into()),
    };
    let root = directory(Some(&models_dir))?;
    fs::create_dir_all(&root).map_err(|error| format!("无法创建模型目录：{error}"))?;
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let key = (root.clone(), model.clone());
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let mut pending = downloads.0.lock().map_err(|error| error.to_string())?;
        if pending.contains_key(&key) {
            return Err("该模型正在下载".into());
        }
        pending.insert(key.clone(), cancelled.clone());
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _task = task;
        let target = root.join(model);
        if installed(&target) {
            return Ok(true);
        }
        if target.exists() {
            return Err("模型目录中已有同名的不完整文件夹，请先移走它或选择其他目录".into());
        }
        let staging = root.join(format!(".download-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&staging).map_err(|error| error.to_string())?;
        let result = (|| {
            for file in FILES {
                let url = format!("{host}/{repo}/resolve/main/{file}");
                if !fetch(&app, &url, &staging.join(file), &request_id, &cancelled)? {
                    return Ok(false);
                }
            }
            if cancelled.load(Ordering::Relaxed) {
                return Ok(false);
            }
            if !installed(&staging) {
                return Err("下载的模型文件不完整".into());
            }
            fs::rename(&staging, &target).map_err(|error| format!("无法保存模型：{error}"))?;
            Ok(true)
        })();
        if staging.is_dir() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    })
    .await;
    downloads
        .0
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&key);
    result.map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn cancel_model_download(
    downloads: State<'_, ModelDownloads>,
    models_dir: String,
    model: String,
) -> Result<(), String> {
    let root = directory(Some(&models_dir))?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if let Some(cancelled) = downloads
        .0
        .lock()
        .map_err(|error| error.to_string())?
        .get(&(root, model))
    {
        cancelled.store(true, Ordering::Relaxed);
    }
    Ok(())
}

fn fetch(
    app: &tauri::AppHandle,
    url: &str,
    file: &Path,
    request_id: &str,
    cancelled: &AtomicBool,
) -> Result<bool, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(false);
    }
    let curl = PathBuf::from(std::env::var_os("SystemRoot").ok_or("缺少 SystemRoot")?)
        .join("System32")
        .join("curl.exe");
    let mut child = Command::new(curl)
        .args([
            "--disable",
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "15",
            "--speed-limit",
            "1",
            "--speed-time",
            "60",
            "--retry",
            "2",
            "--output",
        ])
        .arg(file)
        .arg(url)
        .creation_flags(0x08000000)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法启动下载：{error}"))?;
    let started = Instant::now();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(false);
        }
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            break;
        }
        if file.ends_with("model.bin") {
            let bytes = fs::metadata(file).map_or(0, |entry| entry.len());
            let _ = app.emit(
                "model-download-progress",
                serde_json::json!({
                    "requestId": request_id, "bytes": bytes,
                    "speedMBs": bytes as f64 / 1048576.0 / started.elapsed().as_secs_f64(),
                }),
            );
        }
        thread::sleep(Duration::from_millis(500));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "模型下载失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(true)
}
