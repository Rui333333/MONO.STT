#![windows_subsystem = "windows"]

use mono_stt_installer::{package, system};

use serde::Serialize;
use std::{
    env,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{self, Command},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};
use tauri::{ipc::Channel, AppHandle, LogicalSize, Manager, State, WindowEvent};
use tauri_plugin_dialog::DialogExt;

#[derive(Clone, Serialize)]
struct Progress {
    phase: &'static str,
    percent: Option<u64>,
}

#[derive(Serialize)]
struct Welcome {
    version: String,
    installed: Option<system::Installed>,
    default_directory: PathBuf,
    automatic: bool,
}

struct Prepared {
    folder: tempfile::TempDir,
    version: String,
}

struct Installer {
    package: PathBuf,
    automatic: bool,
    restart: bool,
    requested: Option<PathBuf>,
    running: AtomicBool,
    prepared: Mutex<Option<Prepared>>,
    installed_to: Mutex<Option<PathBuf>>,
}

fn emit(progress: &Channel<Progress>, phase: &'static str, percent: Option<u64>) {
    let _ = progress.send(Progress { phase, percent });
}

fn unpack(bundle: &Path, progress: impl Fn(u64)) -> Result<Prepared, String> {
    let executable = env::current_exe().map_err(|e| e.to_string())?;
    let root = executable.parent().ok_or("无法定位临时安装目录")?;
    let folder = tempfile::Builder::new()
        .prefix("payload-")
        .tempdir_in(root)
        .map_err(|e| e.to_string())?;
    let mut file = package::open(bundle).map_err(|e| e.to_string())?;
    let archive_path = folder.path().join("payload.zip");
    package::copy(&mut file.0, file.1.payload, &archive_path)
        .map_err(|e| format!("无法读取安装资源：{e}"))?;
    let mut zip = zip::ZipArchive::new(File::open(&archive_path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("安装资源损坏：{e}"))?;
    let total: u64 = (0..zip.len())
        .map(|index| zip.by_index(index).map(|entry| entry.size()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .sum();
    let mut written = 0_u64;
    let mut last = 0;
    let mut bytes = vec![0; 256 * 1024];
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|e| e.to_string())?;
        let relative = entry.enclosed_name().ok_or("安装资源包含无效路径")?;
        if entry.is_symlink()
            || !(relative == Path::new(system::EXE)
                || relative == Path::new("version.txt")
                || relative.starts_with("runtime"))
        {
            return Err("安装资源包含不支持的内容。".into());
        }
        let target = folder.path().join(relative);
        if entry.is_dir() {
            fs::create_dir_all(target).map_err(|e| e.to_string())?;
            continue;
        }
        let parent = target.parent().ok_or("安装资源没有目标目录")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut output = File::create_new(target).map_err(|e| e.to_string())?;
        loop {
            let count = entry.read(&mut bytes).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            output
                .write_all(&bytes[..count])
                .map_err(|e| e.to_string())?;
            written += count as u64;
            let next = written.saturating_mul(100) / total.max(1);
            if next != last {
                progress(next);
                last = next;
            }
        }
    }
    drop(zip);
    fs::remove_file(archive_path).map_err(|e| e.to_string())?;
    let version =
        fs::read_to_string(folder.path().join("version.txt")).map_err(|e| e.to_string())?;
    let version = version.trim();
    if version.split('.').count() != 3
        || !version.split('.').all(|part| part.parse::<u32>().is_ok())
        || !folder.path().join(system::EXE).is_file()
        || !folder.path().join("runtime/python.exe").is_file()
    {
        return Err("安装资源缺少应用或运行组件。".into());
    }
    Ok(Prepared {
        folder,
        version: version.into(),
    })
}

fn payload_bytes(path: &Path) -> std::io::Result<u64> {
    let metadata = path.metadata()?;
    if !metadata.is_dir() {
        return Ok(metadata.len());
    }
    fs::read_dir(path)?.try_fold(0, |sum, item| Ok(sum + payload_bytes(&item?.path())?))
}

fn copy_tree(source: &Path, dest: &Path, copied: &mut impl FnMut(u64)) -> std::io::Result<()> {
    if source.is_dir() {
        fs::create_dir(dest)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &dest.join(entry.file_name()), copied)?;
        }
    } else {
        copied(fs::copy(source, dest)?);
    }
    Ok(())
}

fn version_tuple(version: &str) -> Option<[u32; 3]> {
    version
        .split('.')
        .map(str::parse)
        .collect::<Result<Vec<_>, _>>()
        .ok()?
        .try_into()
        .ok()
}

fn check_version(prepared: &Prepared, installed: Option<&system::Installed>) -> Result<(), String> {
    if let (Some(incoming), Some(current)) = (
        version_tuple(&prepared.version),
        installed.and_then(|item| version_tuple(&item.version)),
    ) {
        if incoming < current {
            return Err("电脑上已经安装了更高版本的 MONO.STT。请使用最新的安装包。".into());
        }
    }
    Ok(())
}

fn apply(
    bundle: &Path,
    prepared: &Prepared,
    directory: &Path,
    desktop: bool,
    progress: impl Fn(&'static str, Option<u64>),
) -> Result<(), String> {
    fs::create_dir_all(directory).map_err(|e| format!("无法创建安装目录：{e}"))?;
    system::make_user_storage_writable(directory)?;
    let staging = tempfile::Builder::new()
        .prefix(".mono-install-")
        .tempdir_in(directory)
        .map_err(|e| format!("目标目录不可写：{e}"))?;
    let incoming = staging.path().join("incoming");
    let backup = staging.path().join("backup");
    fs::create_dir(&incoming)
        .and_then(|_| fs::create_dir(&backup))
        .map_err(|e| e.to_string())?;
    let total = [system::EXE, "runtime"]
        .iter()
        .try_fold(0, |sum, name| {
            payload_bytes(&prepared.folder.path().join(name)).map(|size| sum + size)
        })
        .map_err(|e| e.to_string())?
        .max(1);
    let mut copied = 0_u64;
    let mut last_percent = 0_u64;
    progress("copy", Some(0));
    for name in [system::EXE, "runtime"] {
        copy_tree(
            &prepared.folder.path().join(name),
            &incoming.join(name),
            &mut |size| {
                copied += size;
                // Reserve the final portion for replacing files and registering the app.
                let value = (copied.saturating_mul(95) / total).min(95);
                if value != last_percent {
                    progress("copy", Some(value));
                    last_percent = value;
                }
            },
        )
        .map_err(|e| format!("复制安装文件失败：{e}"))?;
    }
    let (mut file, parts) = package::open(bundle).map_err(|e| e.to_string())?;
    package::copy(&mut file, parts.stub, &incoming.join(system::MAINTENANCE))
        .map_err(|e| e.to_string())?;
    progress("close", None);
    system::close_app(directory)?;

    let names = [system::EXE, "runtime", system::MAINTENANCE];
    let mut moved_old = Vec::new();
    let mut moved_new = Vec::new();
    let result = (|| {
        for name in names {
            let existing = directory.join(name);
            if existing.exists() {
                fs::rename(&existing, backup.join(name))
                    .map_err(|e| format!("{name} 仍被占用：{e}"))?;
                moved_old.push(name);
            }
            fs::rename(incoming.join(name), existing)
                .map_err(|e| format!("无法替换 {name}：{e}"))?;
            moved_new.push(name);
        }
        progress("shortcuts", None);
        system::shortcut(directory, desktop)?;
        system::register(directory, &prepared.version)?;
        Ok(())
    })();
    if let Err(error) = result {
        let mut recovery_failed = false;
        for name in moved_new.into_iter().rev() {
            recovery_failed |= fs::rename(directory.join(name), incoming.join(name)).is_err();
        }
        for name in moved_old.into_iter().rev() {
            recovery_failed |= fs::rename(backup.join(name), directory.join(name)).is_err();
        }
        if recovery_failed {
            let recovery = staging.keep();
            return Err(format!("{error}。旧安装备份位于 {}", recovery.display()));
        }
        return Err(error);
    }
    progress("complete", Some(100));
    Ok(())
}

#[tauri::command]
async fn prepare(app: AppHandle, progress: Channel<Progress>) -> Result<Welcome, String> {
    let bundle = app.state::<Installer>().package.clone();
    let prepared = tauri::async_runtime::spawn_blocking(move || {
        unpack(&bundle, |value| emit(&progress, "extract", Some(value)))
    })
    .await
    .map_err(|e| e.to_string())??;
    let state = app.state::<Installer>();
    let response = Welcome {
        version: prepared.version.clone(),
        installed: system::installed(),
        default_directory: state
            .requested
            .clone()
            .map_or_else(system::default_directory, Ok)?,
        automatic: state.automatic,
    };
    *state.prepared.lock().map_err(|e| e.to_string())? = Some(prepared);
    Ok(response)
}

#[tauri::command]
async fn choose_directory(app: AppHandle) -> Result<Option<PathBuf>, String> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    app.dialog()
        .file()
        .set_title("选择 MONO.STT 安装目录")
        .pick_folder(move |path| {
            let _ = sender.send(path.and_then(|path| path.into_path().ok()));
        });
    tauri::async_runtime::spawn_blocking(move || receiver.recv().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn set_page(window: tauri::WebviewWindow, page: String) -> Result<(), String> {
    let size = match page.as_str() {
        "loading" => LogicalSize::new(365.0, 132.0),
        "confirm" => LogicalSize::new(450.0, 252.0),
        "start" | "progress" | "complete" => LogicalSize::new(950.0, 535.0),
        _ => return Err("未知安装页面".into()),
    };
    window
        .set_size(size)
        .and_then(|_| window.center())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal_loading(window: tauri::WebviewWindow) -> Result<(), String> {
    window.show().map_err(|error| error.to_string())?;
    let _ = window.set_focus();
    Ok(())
}

#[tauri::command]
async fn install(
    app: AppHandle,
    directory: Option<PathBuf>,
    desktop: bool,
    overwrite: bool,
    progress: Channel<Progress>,
) -> Result<(), String> {
    let state = app.state::<Installer>();
    if state.running.swap(true, Ordering::AcqRel) {
        return Err("已有安装任务正在进行。".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Installer>();
        let result = (|| {
            let _lock = system::install_lock()?;
            let previous = system::installed();
            if previous.is_some() && !overwrite {
                return Err("请确认覆盖现有安装。".into());
            }
            let directory = system::target(directory.as_deref(), previous.as_ref())?;
            let lock = state.prepared.lock().map_err(|e| e.to_string())?;
            let prepared = lock.as_ref().ok_or("请先完成加载。")?;
            check_version(prepared, previous.as_ref())?;
            apply(
                &state.package,
                prepared,
                &directory,
                desktop,
                |phase, percent| emit(&progress, phase, percent),
            )?;
            *state.installed_to.lock().map_err(|e| e.to_string())? = Some(directory.clone());
            if state.restart {
                Command::new(directory.join(system::EXE))
                    .spawn()
                    .map_err(|e| e.to_string())?;
                app.exit(0);
            }
            Ok(())
        })();
        state.running.store(false, Ordering::Release);
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn finish(app: AppHandle, state: State<'_, Installer>, launch: bool) -> Result<(), String> {
    if state.running.load(Ordering::Acquire) {
        return Err("请等待安装完成。".into());
    }
    if launch {
        let directory = state.installed_to.lock().map_err(|e| e.to_string())?;
        Command::new(directory.as_ref().ok_or("安装尚未完成")?.join(system::EXE))
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    app.exit(0);
    Ok(())
}

#[tauri::command]
fn elevate(
    app: AppHandle,
    state: State<'_, Installer>,
    directory: Option<PathBuf>,
) -> Result<(), String> {
    if state.running.load(Ordering::Acquire) {
        return Err("请等待安装结束。".into());
    }
    system::elevate(&state.package, directory.as_deref())?;
    app.exit(0);
    Ok(())
}

fn main() {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let has = |flag: &str| {
        args.iter()
            .any(|value| value.to_string_lossy().eq_ignore_ascii_case(flag))
    };
    let bundle = args
        .windows(2)
        .find(|pair| pair[0] == "--package")
        .map(|pair| PathBuf::from(&pair[1]));
    let Some(bundle) = bundle else {
        return;
    };
    let restart = has("/r");
    let requested = args
        .windows(2)
        .find(|pair| pair[0] == "--directory")
        .map(|pair| PathBuf::from(&pair[1]));
    if has("/s") || has("/quiet") {
        let exit = (|| {
            let prepared = unpack(&bundle, |_| {})?;
            let _lock = system::install_lock()?;
            let previous = system::installed();
            let directory = system::target(requested.as_deref(), previous.as_ref())?;
            check_version(&prepared, previous.as_ref())?;
            apply(&bundle, &prepared, &directory, false, |_, _| {})?;
            if restart {
                Command::new(directory.join(system::EXE))
                    .spawn()
                    .map_err(|e| e.to_string())?;
            }
            Ok::<_, String>(())
        })();
        process::exit(if exit.is_ok() { 0 } else { 1 });
    }
    tauri::Builder::default()
        .manage(Installer {
            package: bundle,
            automatic: has("/update"),
            restart,
            requested,
            running: AtomicBool::new(false),
            prepared: Mutex::new(None),
            installed_to: Mutex::new(None),
        })
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let Some(window) = app.get_webview_window("main") else {
                return Ok(());
            };
            // Never leave a hidden installer process running after a frontend failure.
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(20));
                if matches!(window.is_visible(), Ok(false)) {
                    window.app_handle().exit(1);
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.state::<Installer>().running.load(Ordering::Acquire) {
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            prepare,
            choose_directory,
            set_page,
            reveal_loading,
            install,
            elevate,
            finish
        ])
        .run(tauri::generate_context!())
        .expect("无法打开安装程序");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::windows::ffi::OsStrExt,
        ptr::{null, null_mut},
    };
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegOverridePredefKey, HKEY,
        HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_OPTION_NON_VOLATILE,
    };

    struct RegistryFixture {
        key: Vec<u16>,
        handle: HKEY,
        app_data: Option<std::ffi::OsString>,
    }

    impl Drop for RegistryFixture {
        fn drop(&mut self) {
            unsafe {
                RegOverridePredefKey(HKEY_CURRENT_USER, null_mut());
                RegCloseKey(self.handle);
                RegDeleteTreeW(HKEY_CURRENT_USER, self.key.as_ptr());
            }
            if let Some(value) = self.app_data.take() {
                env::set_var("APPDATA", value);
            } else {
                env::remove_var("APPDATA");
            }
        }
    }

    #[test]
    fn fresh_install_and_overwrite_keep_records_and_models() {
        let root = tempfile::tempdir().unwrap();
        let test_id = root.path().file_name().unwrap().to_string_lossy();
        let key: Vec<u16> =
            std::ffi::OsStr::new(&format!(r"Software\MONO.STT.InstallerTests\{test_id}"))
                .encode_wide()
                .chain([0])
                .collect();
        let mut handle = null_mut();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_ALL_ACCESS,
                null(),
                &mut handle,
                null_mut(),
            )
        };
        assert_eq!(status, 0);
        let fixture = RegistryFixture {
            key,
            handle,
            app_data: env::var_os("APPDATA"),
        };
        assert_eq!(
            unsafe { RegOverridePredefKey(HKEY_CURRENT_USER, fixture.handle) },
            0
        );
        env::set_var("APPDATA", root.path());
        fs::create_dir_all(root.path().join(r"Microsoft\Windows\Start Menu\Programs")).unwrap();

        let setup = root.path().join("setup.exe");
        let mut bundle = File::create(&setup).unwrap();
        bundle.write_all(b"1234MONOSTT1").unwrap();
        for length in [1_u64; 4] {
            bundle.write_all(&length.to_le_bytes()).unwrap();
        }
        drop(bundle);
        let directory = root.path().join("installed");
        let package_dir = tempfile::tempdir_in(root.path()).unwrap();
        fs::create_dir(package_dir.path().join("runtime")).unwrap();
        let mut prepared = Prepared {
            folder: package_dir,
            version: "0.1.4".into(),
        };
        fs::write(prepared.folder.path().join(system::EXE), b"old program").unwrap();
        fs::write(
            prepared.folder.path().join("runtime/python.exe"),
            b"old runtime",
        )
        .unwrap();

        assert!(system::installed().is_none());
        apply(&setup, &prepared, &directory, false, |_, _| {}).unwrap();
        assert_eq!(system::installed().unwrap().version, "0.1.4");
        assert!(root
            .path()
            .join(r"Microsoft\Windows\Start Menu\Programs\MONO.STT.lnk")
            .exists());

        fs::create_dir(directory.join("data")).unwrap();
        fs::create_dir(directory.join("models")).unwrap();
        fs::write(directory.join("data/mono.db"), b"records").unwrap();
        fs::write(directory.join("models/local-model"), b"model").unwrap();
        prepared.version = "0.1.5".into();
        fs::write(prepared.folder.path().join(system::EXE), b"new program").unwrap();
        fs::write(
            prepared.folder.path().join("runtime/python.exe"),
            b"new runtime",
        )
        .unwrap();
        let updates = std::cell::RefCell::new(Vec::new());
        apply(&setup, &prepared, &directory, false, |phase, percent| {
            updates.borrow_mut().push((phase, percent));
        })
        .unwrap();
        let updates = updates.into_inner();
        let (copy_updates, final_updates) = updates.split_at(updates.len() - 3);
        assert_eq!(copy_updates.first(), Some(&("copy", Some(0))));
        assert_eq!(copy_updates.last(), Some(&("copy", Some(95))));
        assert!(copy_updates.windows(2).all(|pair| pair[0].1 < pair[1].1));
        assert_eq!(
            final_updates,
            &[
                ("close", None),
                ("shortcuts", None),
                ("complete", Some(100))
            ]
        );
        assert_eq!(system::installed().unwrap().version, "0.1.5");
        assert_eq!(
            fs::read(directory.join(system::EXE)).unwrap(),
            b"new program"
        );
        assert_eq!(
            fs::read(directory.join("runtime/python.exe")).unwrap(),
            b"new runtime"
        );
        assert_eq!(
            fs::read(directory.join("data/mono.db")).unwrap(),
            b"records"
        );
        assert_eq!(
            fs::read(directory.join("models/local-model")).unwrap(),
            b"model"
        );
        assert!(!fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .any(|file| file
                .file_name()
                .to_string_lossy()
                .starts_with(".mono-install-")));
    }
}
