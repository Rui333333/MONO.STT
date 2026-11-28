use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

struct ModelFile {
    name: &'static str,
    size: u64,
    sha256: &'static str,
}

struct Model {
    id: &'static str,
    repository: &'static str,
    revision: &'static str,
    files: [ModelFile; 5],
}

const PREPROCESSOR: ModelFile = ModelFile {
    name: "preprocessor_config.json",
    size: 340,
    sha256: "7ccc62c6f2765af1f3b46c00c9b5894426835a05021c8b9c01eecb6dfb542711",
};
const VOCABULARY: ModelFile = ModelFile {
    name: "vocabulary.json",
    size: 1_068_114,
    sha256: "c69260f2ab26d659b7c398f9a2b2b48ed0df16c3b47d7326782fd9cba71690c1",
};

// Digests are pinned to the Hugging Face Git revision below. The weight
// digest is the LFS SHA-256, and JSON digests are of the raw revision files.
const MODELS: [Model; 2] = [
    Model {
        id: "faster-whisper-large-v3-turbo",
        repository: "dropbox-dash/faster-whisper-large-v3-turbo",
        revision: "0a363e9161cbc7ed1431c9597a8ceaf0c4f78fcf",
        files: [
            ModelFile {
                name: "config.json",
                size: 2_263,
                sha256: "b0253ea6c0d3bea6b1e19e91a02acfd3b53f4467362efcb5a3e6b16c9b3a9b7e",
            },
            ModelFile {
                name: "tokenizer.json",
                size: 2_710_337,
                sha256: "297b13372ac43916285644fb9687add3cc62ee2a1adb60da3dc25cc94c1871fd",
            },
            VOCABULARY,
            PREPROCESSOR,
            ModelFile {
                name: "model.bin",
                size: 1_617_884_929,
                sha256: "e76620f83d5f5b69efd3d87e3dc180c1bd21df9fbebacfd4335e5e1efcc018da",
            },
        ],
    },
    Model {
        id: "faster-whisper-large-v3",
        repository: "Systran/faster-whisper-large-v3",
        revision: "edaa852ec7e145841d8ffdb056a99866b5f0a478",
        files: [
            ModelFile {
                name: "config.json",
                size: 2_394,
                sha256: "a9306624f5ec14270a014b647e5c316b6e03a662c369758d1b90697a7b0655b9",
            },
            ModelFile {
                name: "tokenizer.json",
                size: 2_480_617,
                sha256: "6d8cbd7cd0d8d5815e478dac67b85a26bbe77c1f5e0c6d76d1ce2abc0e5f21ca",
            },
            VOCABULARY,
            PREPROCESSOR,
            ModelFile {
                name: "model.bin",
                size: 3_087_284_237,
                sha256: "69f74147e3334731bc3a76048724833325d2ec74642fb52620eda87352e3d4f1",
            },
        ],
    },
];

#[derive(PartialEq, Eq)]
struct FileStamp {
    modified: SystemTime,
    created: Option<SystemTime>,
}

type VerificationKey = (PathBuf, &'static str);
#[derive(Default)]
pub struct ModelVerifications(Mutex<HashMap<VerificationKey, Vec<FileStamp>>>);

impl ModelVerifications {
    fn check(&self, path: &Path, model: &Model) -> Result<(), String> {
        let path = path
            .canonicalize()
            .map_err(|err| format!("找不到模型目录：{err}"))?;
        let files = file_stamps(&path, &model.files)?;
        let key = (path.clone(), model.id);
        {
            let mut cache = self.0.lock().map_err(|err| err.to_string())?;
            if cache.get(&key) == Some(&files) {
                return Ok(());
            }
            cache.remove(&key);
        }
        verify_package(&path, &model.files, &files, &AtomicBool::new(false))?;
        self.remember(key, files);
        Ok(())
    }

    fn remember(&self, key: VerificationKey, files: Vec<FileStamp>) {
        if let Ok(mut cache) = self.0.lock() {
            cache.insert(key, files);
        }
    }
}

fn file_stamps(path: &Path, files: &[ModelFile]) -> Result<Vec<FileStamp>, String> {
    files
        .iter()
        .map(|file| {
            let metadata = fs::metadata(path.join(file.name))
                .map_err(|err| format!("无法读取模型文件 {}：{err}", file.name))?;
            if !metadata.is_file() || metadata.len() != file.size {
                return Err(format!("模型大小不匹配：{}", file.name));
            }
            Ok(FileStamp {
                modified: metadata.modified().map_err(|err| err.to_string())?,
                created: metadata.created().ok(),
            })
        })
        .collect()
}

fn verify_package(
    path: &Path,
    files: &[ModelFile],
    before: &[FileStamp],
    cancelled: &AtomicBool,
) -> Result<bool, String> {
    let mut buffer = vec![0; 1024 * 1024];
    for file in files {
        let mut input = File::open(path.join(file.name))
            .map_err(|err| format!("无法打开模型文件 {}：{err}", file.name))?;
        let mut hash = Sha256::new();
        let mut bytes = 0;
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(false);
            }
            let count = input
                .read(&mut buffer)
                .map_err(|err| format!("无法读取模型文件 {}：{err}", file.name))?;
            if count == 0 {
                break;
            }
            bytes += count as u64;
            if bytes > file.size {
                return Err(format!("校验时模型大小改变：{}", file.name));
            }
            hash.update(&buffer[..count]);
        }
        if bytes != file.size || format!("{:x}", hash.finalize()) != file.sha256 {
            return Err(format!(
                "模型文件完整性校验失败：{}（SHA-256 不匹配）",
                file.name
            ));
        }
    }
    if file_stamps(path, files)? != before {
        return Err("校验期间模型文件发生变化，请重试".into());
    }
    Ok(!cancelled.load(Ordering::Relaxed))
}

fn model_spec(id: &str) -> Result<&'static Model, String> {
    MODELS
        .iter()
        .find(|model| model.id == id)
        .ok_or_else(|| "未知模型".into())
}

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
    Ok(crate::platform::data_root()?.join("models"))
}

pub fn verify_installed(app: &tauri::AppHandle, path: &Path, id: &str) -> Result<(), String> {
    let spec = model_spec(id)?;
    app.state::<ModelVerifications>().check(path, spec)
}

pub fn valid_model(model: &str) -> bool {
    model_spec(model).is_ok()
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
pub async fn list_local_models(
    app: tauri::AppHandle,
    models_dir: String,
) -> Result<Vec<&'static str>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = directory(Some(&models_dir))?;
        if root.exists() {
            fs::read_dir(&root).map_err(|error| format!("无法读取模型目录：{error}"))?;
        }
        let verification = app.state::<ModelVerifications>();
        Ok(MODELS
            .iter()
            .filter(|model| verification.check(&root.join(model.id), model).is_ok())
            .map(|model| model.id)
            .collect())
    })
    .await
    .map_err(|error| error.to_string())?
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
    let spec = model_spec(&model)?;
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
        let target = root.join(spec.id);
        let verification = app.state::<ModelVerifications>();
        if fs::symlink_metadata(&target).is_ok() {
            verification
                .check(&target, spec)
                .map_err(|err| format!("{err}。请移走原模型目录或选择其他目录后重新下载。"))?;
            return Ok(!cancelled.load(Ordering::Relaxed));
        }
        let staging = tempfile::Builder::new()
            .prefix(".download-")
            .tempdir_in(&root)
            .map_err(|error| format!("无法创建下载目录：{error}"))?;
        for file in &spec.files {
            let url = format!(
                "{host}/{}/resolve/{}/{}",
                spec.repository, spec.revision, file.name
            );
            if !fetch(
                &app,
                &url,
                &staging.path().join(file.name),
                file.size,
                &request_id,
                &cancelled,
            )? {
                return Ok(false);
            }
        }
        let _ = app.emit(
            "model-download-progress",
            serde_json::json!({
                "requestId": request_id, "phase": "verifying",
            }),
        );
        let files = file_stamps(staging.path(), &spec.files)?;
        if !verify_package(staging.path(), &spec.files, &files, &cancelled)? {
            return Ok(false);
        }
        if fs::symlink_metadata(&target).is_ok() {
            return Err("目标模型目录已存在，请重新扫描模型目录".into());
        }
        fs::rename(staging.path(), &target).map_err(|error| format!("无法保存模型：{error}"))?;
        if let Ok(target) = target.canonicalize() {
            verification.remember((target, spec.id), files);
        }
        Ok(true)
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
    expected_size: u64,
    request_id: &str,
    cancelled: &AtomicBool,
) -> Result<bool, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(false);
    }
    let mut child = crate::platform::command(crate::platform::curl()?)
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
            "--max-filesize",
        ])
        .arg(expected_size.to_string())
        .arg("--output")
        .arg(file)
        .arg(url)
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
        let bytes = fs::metadata(file).map_or(0, |entry| entry.len());
        if bytes > expected_size {
            let _ = child.kill();
            let _ = child.wait();
            return Err("模型下载超过预期大小".into());
        }
        if file.ends_with("model.bin") {
            let _ = app.emit(
                "model-download-progress",
                serde_json::json!({
                    "requestId": request_id, "phase": "downloading", "bytes": bytes,
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
    if fs::metadata(file).map_err(|error| error.to_string())?.len() != expected_size {
        return Err(format!(
            "下载的模型大小不匹配：{}",
            file.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: ModelFile = ModelFile {
        name: "config.json",
        size: 3,
        sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    };

    #[test]
    fn checks_size_sha256_and_cancellation_without_accepting_bad_data() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path();
        let files = [FILE];
        assert!(file_stamps(path, &files).is_err());
        fs::write(path.join(FILE.name), "abcd").unwrap();
        assert!(file_stamps(path, &files).is_err());
        fs::write(path.join(FILE.name), "abc").unwrap();
        let stamps = file_stamps(path, &files).unwrap();
        let cancelled = AtomicBool::new(false);
        assert!(verify_package(path, &files, &stamps, &cancelled).unwrap());
        cancelled.store(true, Ordering::Relaxed);
        assert!(!verify_package(path, &files, &stamps, &cancelled).unwrap());
        cancelled.store(false, Ordering::Relaxed);
        fs::write(path.join(FILE.name), "abd").unwrap();
        let stamps = file_stamps(path, &files).unwrap();
        assert!(verify_package(path, &files, &stamps, &cancelled)
            .unwrap_err()
            .contains("SHA-256"));
    }
}
