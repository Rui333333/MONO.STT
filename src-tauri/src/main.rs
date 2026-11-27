#![cfg_attr(windows, windows_subsystem = "windows")]

mod batch_export;
mod feishu;
mod models;
mod platform;
mod store;
mod updates;

use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
};
use tauri::{ipc::Channel, path::BaseDirectory, Manager};
use tauri_plugin_dialog::DialogExt;

#[cfg(windows)]
const INFERENCE_WORKER: &str = include_str!("../../backend/transcribe.py");

#[tauri::command]
fn open_feishu_credentials() -> Result<(), String> {
    platform::open_url("https://open.feishu.cn/app")
}

#[tauri::command]
fn open_cuda_download() -> Result<(), String> {
    #[cfg(windows)]
    return platform::open_url(
        "https://developer.nvidia.com/cuda-12-9-0-download-archive?target_os=Windows",
    );
    #[cfg(target_os = "macos")]
    return Err("macOS 预览版使用 CPU 转录".into());
}

#[cfg(windows)]
fn inference_python(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Ok(runtime) = app
        .path()
        .resolve("runtime/python.exe", BaseDirectory::Resource)
    {
        if runtime.is_file() {
            return Ok(runtime);
        }
    }
    Ok(
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("缺少 LOCALAPPDATA")?)
            .join(r"InkVoice\runtime\Scripts\python.exe"),
    )
}

#[tauri::command]
async fn gpu_status(app: tauri::AppHandle) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || probe_gpu(app))
        .await
        .map_err(|error| error.to_string())
}

#[cfg(windows)]
fn probe_gpu(app: tauri::AppHandle) -> Value {
    let system = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32");
    let mut adapters = Vec::new();
    if let Ok(output) = platform::command(system.join("nvidia-smi.exe"))
        .args([
            "--query-gpu=name,memory.total,compute_cap",
            "--format=csv,noheader,nounits",
        ])
        .output()
    {
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let fields: Vec<_> = line.split(',').map(str::trim).collect();
                if let [name, memory, capability] = fields.as_slice() {
                    adapters.push(json!({
                        "name": name, "memoryMiB": memory.parse::<u32>().ok(),
                        "capability": capability,
                    }));
                }
            }
        }
    }
    if adapters.is_empty() {
        if let Ok(output) = platform::command(system.join(r"WindowsPowerShell\v1.0\powershell.exe"))
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Console]::OutputEncoding = [Text.Encoding]::UTF8; (Get-CimInstance Win32_VideoController).Name",
            ])
                .output()
        {
            if output.status.success() {
                adapters.extend(
                    String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .map(|name| json!({ "name": name })),
                );
            }
        }
    }
    let mut status = inference_python(&app)
        .and_then(|python| {
            if !python.is_file() {
                return Err("推理环境尚未安装".into());
            }
            let output = platform::command(python)
                .args(["-I", "-B", "-c", INFERENCE_WORKER, "--probe-cuda"])
                .output()
                .map_err(|error| error.to_string())?;
            let probe: Value = serde_json::from_slice(&output.stdout)
                .map_err(|_| "无法读取 CUDA 推理检测结果".to_string())?;
            if !output.status.success() {
                return Err(probe["message"].as_str().unwrap_or("CUDA 检测失败").into());
            }
            Ok(probe)
        })
        .unwrap_or_else(|reason| json!({ "cudaAvailable": false, "reason": reason }));
    status["adapters"] = json!(adapters);
    status
}

#[cfg(target_os = "macos")]
fn probe_gpu(_: tauri::AppHandle) -> Value {
    json!({"platform":"macos", "cudaAvailable":false, "adapters":[], "computeTypes":["int8"]})
}

fn worker_command(app: &tauri::AppHandle) -> Result<Command, String> {
    #[cfg(windows)]
    {
        let mut command = platform::command(inference_python(app)?);
        command.args(["-I", "-B", "-u", "-c", INFERENCE_WORKER]);
        Ok(command)
    }
    #[cfg(target_os = "macos")]
    {
        let worker = app
            .path()
            .resolve("runtime/mono-stt-worker", BaseDirectory::Resource)
            .map_err(|error| error.to_string())?;
        if !worker.is_file() {
            return Err("未找到内置的 macOS 转录引擎，请从安装包重新安装".into());
        }
        Ok(platform::command(worker))
    }
}

#[tauri::command]
async fn pick_files(app: tauri::AppHandle) -> Vec<String> {
    app.dialog()
        .file()
        .add_filter(
            "音频和视频",
            &[
                "mp4", "mov", "mkv", "webm", "avi", "m4v", "mp3", "wav", "m4a", "aac", "flac",
                "ogg", "opus", "wma", "m4b",
            ],
        )
        .blocking_pick_files()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

#[tauri::command]
async fn export_file(app: tauri::AppHandle, name: String, content: String) -> Result<bool, String> {
    let Some(path) = app.dialog().file().set_file_name(name).blocking_save_file() else {
        return Ok(false);
    };
    fs::write(path.into_path().map_err(|e| e.to_string())?, content).map_err(|e| e.to_string())?;
    Ok(true)
}

const TRANSCRIPTION_CANCELLED: &str = "TRANSCRIPTION_CANCELLED";

#[derive(Default)]
struct TranscriptionProcess {
    child: Option<Child>,
    cancelled: bool,
    finished: bool,
}

impl TranscriptionProcess {
    fn cancel(&mut self) -> Result<bool, String> {
        if self.finished {
            return Ok(false);
        }
        if let Some(child) = &mut self.child {
            if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                return Ok(false);
            }
            child.kill().map_err(|e| e.to_string())?;
        }
        self.cancelled = true;
        Ok(true)
    }
}

#[cfg(test)]
mod transcription_tests {
    use super::*;

    #[test]
    fn cancellation_stops_a_running_worker_and_is_safe_before_start() {
        let mut pending = TranscriptionProcess::default();
        assert!(pending.cancel().unwrap());
        assert!(pending.cancelled);

        #[cfg(windows)]
        let mut command = {
            let mut command = platform::command("powershell.exe");
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ]);
            command
        };
        #[cfg(target_os = "macos")]
        let mut command = {
            let mut command = Command::new("/bin/sleep");
            command.arg("30");
            command
        };
        let mut running = TranscriptionProcess {
            child: Some(command.spawn().unwrap()),
            ..Default::default()
        };
        assert!(running.cancel().unwrap());
        assert!(!running.child.take().unwrap().wait().unwrap().success());
        assert!(running.cancelled);
        running.finished = true;
        assert!(!running.cancel().unwrap());
    }
}

type ProcessHandle = Arc<Mutex<TranscriptionProcess>>;

#[derive(Default)]
struct Transcriptions(Mutex<HashMap<uuid::Uuid, ProcessHandle>>);

fn discard_interrupted_audio(data_dir: &Path) {
    let Ok(entries) = fs::read_dir(data_dir.join("audio")) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "transcribing")
            && path
                .file_stem()
                .and_then(|name| name.to_str())
                .is_some_and(|name| uuid::Uuid::parse_str(name).is_ok())
        {
            let _ = fs::remove_file(path);
        }
    }
}

#[tauri::command]
fn cancel_transcription(app: tauri::AppHandle, job_id: String) -> Result<bool, String> {
    let job_id = uuid::Uuid::parse_str(&job_id).map_err(|e| e.to_string())?;
    let process = app
        .state::<Transcriptions>()
        .0
        .lock()
        .unwrap()
        .get(&job_id)
        .cloned();
    match process {
        Some(process) => process.lock().unwrap().cancel(),
        None => Ok(false),
    }
}

#[tauri::command]
async fn transcribe(
    app: tauri::AppHandle,
    job_id: String,
    source: String,
    model: String,
    options: Value,
    events: Channel<Value>,
) -> Result<Value, String> {
    let job_id = uuid::Uuid::parse_str(&job_id).map_err(|e| e.to_string())?;
    let processes = app.state::<Transcriptions>();
    let process = ProcessHandle::default();
    {
        let mut jobs = processes.0.lock().unwrap();
        if jobs.contains_key(&job_id) {
            return Err("此转录任务已在运行".into());
        }
        jobs.insert(job_id, process.clone());
    }
    let _ = events.send(json!({ "kind": "ready" }));
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        run_worker(
            worker_app,
            source,
            model,
            options,
            &process,
            Some((job_id, events)),
        )
    })
    .await;
    processes.0.lock().unwrap().remove(&job_id);
    result.map_err(|e| e.to_string())?
}

fn run_worker(
    app: tauri::AppHandle,
    source: String,
    model: String,
    options: Value,
    process: &Mutex<TranscriptionProcess>,
    live: Option<(uuid::Uuid, Channel<Value>)>,
) -> Result<Value, String> {
    let _task = updates::task(&app)?;
    if !models::valid_model(&model) {
        return Err("未知模型".into());
    }
    let model_dir = models::directory(options["modelsDir"].as_str())?.join(model);
    if !models::installed(&model_dir) {
        return Err(format!("未找到完整的模型：{}", model_dir.display()));
    }
    let audio_dir = app.state::<store::Store>().data_dir().join("audio");
    fs::create_dir_all(&audio_dir).map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let wav = audio_dir.join(format!("{id}.wav"));
    let staging = wav.with_extension("transcribing");
    let record_id = live.as_ref().map_or(id, |(job_id, _)| job_id.to_string());
    let mut command = worker_command(&app)?;
    command
        .arg(source)
        .arg(model_dir)
        .arg(&staging)
        .arg(options.to_string())
        .stdout(Stdio::piped());
    let stdout = {
        let mut state = process.lock().unwrap();
        if state.cancelled {
            return Err(TRANSCRIPTION_CANCELLED.into());
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("无法启动推理环境：{e}"))?;
        let stdout = child.stdout.take().expect("worker stdout is piped");
        state.child = Some(child);
        stdout
    };
    let mut result = None;
    let mut error = String::new();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Ok(mut event) = serde_json::from_str::<Value>(&line) {
            match event["kind"].as_str() {
                Some("result") => {
                    event["audio"] = json!(wav);
                    event["id"] = json!(record_id);
                    result = Some(event);
                }
                Some("error") => error = event["message"].as_str().unwrap_or_default().to_string(),
                Some("status" | "segment" | "reset" | "metadata") => {
                    if let Some((_, channel)) = &live {
                        let _ = channel.send(event);
                    }
                }
                _ => {}
            }
        }
    }
    let (cancelled, mut child) = {
        let mut state = process.lock().unwrap();
        state.finished = true;
        (
            state.cancelled,
            state.child.take().expect("worker was started"),
        )
    };
    let status = child.wait();
    if !cancelled && status.as_ref().is_ok_and(|status| status.success()) {
        if let Some(value) = result {
            if live.is_none() {
                fs::remove_file(&staging).map_err(|e| e.to_string())?;
            } else {
                if let Err(error) = fs::rename(&staging, &wav) {
                    let _ = fs::remove_file(&staging);
                    return Err(error.to_string());
                }
                if let Err(error) = app.state::<store::Store>().allow_playback(&app, &wav) {
                    let _ = fs::remove_file(&wav);
                    return Err(error);
                }
            }
            return Ok(value);
        }
    }
    let _ = fs::remove_file(staging);
    if cancelled {
        return Err(TRANSCRIPTION_CANCELLED.into());
    }
    status.map_err(|e| e.to_string())?;
    Err(if error.is_empty() {
        "推理进程意外退出，请检查运行环境".into()
    } else {
        error
    })
}

#[tauri::command]
async fn onboarding_sample(
    app: tauri::AppHandle,
    model: String,
    options: Value,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = app.state::<store::Store>().data_dir().join("setup");
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let input = dir.join(format!("{}.wav", uuid::Uuid::new_v4()));
        fs::write(&input, include_bytes!("../../ui/sample_zh.wav")).map_err(|e| e.to_string())?;
        let started = std::time::Instant::now();
        let result = run_worker(
            app,
            input.to_string_lossy().into_owned(),
            model,
            options,
            &Mutex::default(),
            None,
        );
        let _ = fs::remove_file(input);
        let mut result = result?;
        if result["segments"].as_array().is_none_or(Vec::is_empty) {
            return Err("示例音频未识别出文字，请检查模型或更换运行设备".into());
        }
        result["elapsed"] = json!(started.elapsed().as_secs_f64());
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn stage_imported_media(
    app: tauri::AppHandle,
    request: tauri::ipc::Request<'_>,
) -> Result<String, String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("导入的媒体数据无效".into());
    };
    let dir = app.state::<store::Store>().data_dir().join("imports");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.media", uuid::Uuid::new_v4()));
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}
#[tauri::command]
fn show_startup_window(window: tauri::WebviewWindow) -> Result<(), String> {
    window.show().map_err(|e| e.to_string())
}

#[tauri::command]
async fn init_store(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        static INITIALIZING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = INITIALIZING.lock().map_err(|e| e.to_string())?;
        if app.try_state::<store::Store>().is_none() {
            let store = store::Store::open(&app)?;
            discard_interrupted_audio(store.data_dir());
            app.manage(store);
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn restore_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[cfg(windows)]
fn set_shell_icons(window: &tauri::WebviewWindow) -> Result<(), Box<dyn std::error::Error>> {
    use windows_sys::Win32::{
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
            WindowsAndMessaging::*,
        },
    };
    let hwnd = window.hwnd()?.0;
    // Tauri embeds the multi-resolution application icon as IDI_APPLICATION.
    // Shared resource handles remain valid for the lifetime of the process.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let dpi = GetDpiForWindow(hwnd);
        for (slot, x, y) in [
            (ICON_BIG, SM_CXICON, SM_CYICON),
            (ICON_SMALL, SM_CXSMICON, SM_CYSMICON),
        ] {
            let icon = LoadImageW(
                module,
                IDI_APPLICATION,
                IMAGE_ICON,
                GetSystemMetricsForDpi(x, dpi),
                GetSystemMetricsForDpi(y, dpi),
                LR_SHARED,
            );
            if icon.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            SendMessageW(hwnd, WM_SETICON, slot as usize, icon as isize);
        }
    }
    Ok(())
}

fn main() {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    tauri::Builder::default()
        .manage(feishu::FeishuPublish::default())
        .manage(models::ModelDownloads::default())
        .manage(updates::Activity::default())
        .manage(Transcriptions::default())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            restore_main(app)
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            #[cfg(windows)]
            if let Some(window) = app.get_webview_window("main") {
                set_shell_icons(&window)?;
            }
            let open = MenuItem::with_id(app, "open", "打开 MONO.STT", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::with_id("main-tray")
                .icon(tauri::include_image!("icons/tray.png"))
                .tooltip("MONO.STT")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => restore_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        restore_main(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            show_startup_window,
            init_store,
            updates::app_update_info,
            updates::check_app_update,
            updates::install_app_update,
            onboarding_sample,
            store::complete_onboarding,
            pick_files,
            export_file,
            batch_export::export_zip,
            transcribe,
            cancel_transcription,
            gpu_status,
            open_cuda_download,
            open_feishu_credentials,
            feishu::feishu_status,
            feishu::feishu_save_credentials,
            feishu::feishu_publish,
            feishu::feishu_delete_document,
            feishu::feishu_open_document,
            feishu::feishu_open_permissions,
            stage_imported_media,
            store::store_load,
            store::store_audio_status,
            store::store_save,
            store::store_delete_records,
            store::store_kv_get,
            store::store_kv_set,
            store::store_kv_delete,
            store::store_import,
            store::store_stats,
            models::default_models_dir,
            models::pick_models_dir,
            models::list_local_models,
            models::download_model,
            models::cancel_model_download
        ])
        .build(tauri::generate_context!())
        .expect("无法启动墨声")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                for process in app.state::<Transcriptions>().0.lock().unwrap().values() {
                    let _ = process.lock().unwrap().cancel();
                }
            }
        });
}
