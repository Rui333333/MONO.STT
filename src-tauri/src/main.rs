#![windows_subsystem = "windows"]

mod feishu;
mod models;

use serde_json::{json, Value};
use std::os::windows::process::CommandExt;
use std::{
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
};
use tauri::{path::BaseDirectory, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

const INFERENCE_WORKER: &str = include_str!("../../backend/transcribe.py");

#[tauri::command]
fn open_feishu_credentials() -> Result<(), String> {
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let url: Vec<u16> = "https://open.feishu.cn/app"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            url.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if (result as isize) <= 32 {
        return Err("无法打开飞书开发者后台".into());
    }
    Ok(())
}

#[tauri::command]
fn open_cuda_download() -> Result<(), String> {
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let url: Vec<u16> =
        "https://developer.nvidia.com/cuda-12-9-0-download-archive?target_os=Windows"
            .encode_utf16()
            .chain(Some(0))
            .collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            url.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if (result as isize) <= 32 {
        return Err("无法打开 NVIDIA 下载页".into());
    }
    Ok(())
}

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

fn probe_gpu(app: tauri::AppHandle) -> Value {
    let system = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32");
    let mut adapters = Vec::new();
    if let Ok(output) = Command::new(system.join("nvidia-smi.exe"))
        .args([
            "--query-gpu=name,memory.total,compute_cap",
            "--format=csv,noheader,nounits",
        ])
        .creation_flags(0x08000000)
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
        if let Ok(output) = Command::new(system.join(r"WindowsPowerShell\v1.0\powershell.exe"))
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Console]::OutputEncoding = [Text.Encoding]::UTF8; (Get-CimInstance Win32_VideoController).Name",
            ])
            .creation_flags(0x08000000)
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
            let output = Command::new(python)
                .args(["-I", "-B", "-c", INFERENCE_WORKER, "--probe-cuda"])
                .creation_flags(0x08000000)
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

#[tauri::command]
async fn transcribe(
    app: tauri::AppHandle,
    source: String,
    model: String,
    options: Value,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || run_worker(app, source, model, options))
        .await
        .map_err(|e| e.to_string())?
}

fn run_worker(
    app: tauri::AppHandle,
    source: String,
    model: String,
    options: Value,
) -> Result<Value, String> {
    if !models::valid_model(&model) {
        return Err("未知模型".into());
    }
    let model_dir = models::directory(options["modelsDir"].as_str())?.join(model);
    if !models::installed(&model_dir) {
        return Err(format!("未找到完整的模型：{}", model_dir.display()));
    }
    let data = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    let audio_dir = data.join("audio");
    fs::create_dir_all(&audio_dir).map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let wav = audio_dir.join(format!("{id}.wav"));
    let mut child = Command::new(inference_python(&app)?)
        .arg("-I")
        .arg("-B")
        .arg("-u")
        .arg("-c")
        .arg(INFERENCE_WORKER)
        .arg(source)
        .arg(model_dir)
        .arg(&wav)
        .arg(options.to_string())
        .creation_flags(0x08000000)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动推理环境：{e}"))?;
    let mut result = None;
    let mut error = String::new();
    for line in BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map_while(Result::ok)
    {
        if let Ok(mut event) = serde_json::from_str::<Value>(&line) {
            match event["kind"].as_str() {
                Some("result") => {
                    event["audio"] = json!(wav);
                    event["id"] = json!(id);
                    result = Some(event);
                }
                Some("error") => error = event["message"].as_str().unwrap_or_default().to_string(),
                Some("status") => {
                    let _ = app.emit("progress", event);
                }
                _ => {}
            }
        }
    }
    if child.wait().map_err(|e| e.to_string())?.success() {
        if let Some(value) = result {
            return Ok(value);
        }
    }
    let _ = fs::remove_file(wav);
    Err(if error.is_empty() {
        "推理进程意外退出，请检查运行环境".into()
    } else {
        error
    })
}

#[tauri::command]
fn store_recording(
    app: tauri::AppHandle,
    request: tauri::ipc::Request<'_>,
) -> Result<String, String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("录音数据无效".into());
    };
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("recordings");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.webm", uuid::Uuid::new_v4()));
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
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
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            restore_main(app)
        }))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            #[cfg(windows)]
            if let Some(window) = app.get_webview_window("main") {
                set_shell_icons(&window)?;
            }
            let open = MenuItem::with_id(app, "open", "打开 MONO.STT", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::with_id("main-tray")
                .icon(tauri::include_image!("icons/32x32.png"))
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
            pick_files,
            export_file,
            transcribe,
            gpu_status,
            open_cuda_download,
            open_feishu_credentials,
            feishu::feishu_status,
            feishu::feishu_save_credentials,
            feishu::feishu_publish,
            feishu::feishu_open_document,
            store_recording,
            models::default_models_dir,
            models::pick_models_dir,
            models::list_local_models,
            models::download_model,
            models::cancel_model_download
        ])
        .run(tauri::generate_context!())
        .expect("无法启动墨声");
}
