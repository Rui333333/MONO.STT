use std::{
    ffi::OsStr,
    fs, io,
    os::windows::ffi::OsStrExt,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    ptr::{null, null_mut},
};
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED, STGM_READ,
        },
        UI::Shell::{IShellLinkW, ShellLink, SLGP_RAWPATH},
    },
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, HANDLE,
        INVALID_HANDLE_VALUE,
    },
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        Registry::{
            RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW, HKEY,
            HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
            RRF_RT_REG_SZ,
        },
        Threading::{
            CreateMutexW, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
            WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            PROCESS_TERMINATE,
        },
    },
    UI::{
        Shell::{IsUserAnAdmin, ShellExecuteW},
        WindowsAndMessaging::{MessageBoxW, SW_SHOWNORMAL},
    },
};

const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\MONO.STT";
const LOCATION: &str = r"Software\inkvoice\MONO.STT";
const WEBVIEW: &str =
    r"Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
const WEBVIEW_MACHINE: &str =
    r"Software\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
pub const EXE: &str = "mono-stt.exe";
pub const MAINTENANCE: &str = "mono-stt-maintenance.exe";
pub const UNINSTALL_UI: &str = "mono-stt-uninstall-ui.exe";
pub const APP_ICON: &str = "mono-stt-app.ico";

#[derive(Clone, serde::Serialize)]
pub struct Installed {
    pub directory: PathBuf,
    pub version: String,
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain([0]).collect()
}

fn reg_get(root: HKEY, key: &str, name: &str) -> Option<String> {
    let mut buffer = vec![0u16; 32768];
    let mut size = (buffer.len() * 2) as u32;
    let result = unsafe {
        RegGetValueW(
            root,
            wide(key).as_ptr(),
            wide(name).as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if result != 0 {
        return None;
    }
    let end = buffer.iter().position(|value| *value == 0)?;
    Some(String::from_utf16_lossy(&buffer[..end]))
}

fn reg_set(key: &str, rows: &[(&str, &str)]) -> Result<(), String> {
    let mut handle = null_mut();
    let result = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(key).as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            null(),
            &mut handle,
            null_mut(),
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32).to_string());
    }
    let result = rows.iter().try_for_each(|(name, value)| {
        let value = wide(value);
        let result = unsafe {
            RegSetValueExW(
                handle,
                wide(name).as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(result as i32).to_string())
        }
    });
    unsafe { RegCloseKey(handle) };
    result
}

pub fn webview_installed() -> bool {
    [
        (HKEY_CURRENT_USER, WEBVIEW),
        (HKEY_LOCAL_MACHINE, WEBVIEW_MACHINE),
        (HKEY_LOCAL_MACHINE, WEBVIEW),
    ]
    .iter()
    .any(|&(key, path)| {
        reg_get(key, path, "pv").is_some_and(|v| {
            v.split('.')
                .any(|part| part.parse::<u32>().is_ok_and(|n| n > 0))
        })
    })
}

pub fn installed() -> Option<Installed> {
    let directory = existing_directory(
        [
            reg_get(HKEY_CURRENT_USER, UNINSTALL, "InstallLocation"),
            reg_get(HKEY_CURRENT_USER, LOCATION, ""),
        ]
        .into_iter()
        .flatten(),
    )?;
    Some(Installed {
        directory,
        version: reg_get(HKEY_CURRENT_USER, UNINSTALL, "DisplayVersion").unwrap_or_default(),
    })
}

fn existing_directory(paths: impl IntoIterator<Item = String>) -> Option<PathBuf> {
    paths.into_iter().find_map(|value| {
        let value = value.trim();
        // Older installers stored InstallLocation with surrounding quotes.
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(value);
        let directory = PathBuf::from(value);
        (directory.is_absolute() && directory.join(EXE).is_file()).then_some(directory)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_install_accepts_local_signing_folder_only_at_default_path() {
        let root = tempfile::tempdir().unwrap();
        let default = root.path().join("MONO.STT");
        let custom = root.path().join("Other");
        fs::create_dir_all(default.join("Signing")).unwrap();
        fs::create_dir_all(default.join("data")).unwrap();
        fs::create_dir_all(custom.join("Signing")).unwrap();

        assert!(check_fresh_directory(&default, &default).is_ok());
        assert!(check_fresh_directory(&custom, &default)
            .unwrap_err()
            .contains("Signing"));
        fs::write(default.join("unrelated.txt"), []).unwrap();
        assert!(check_fresh_directory(&default, &default)
            .unwrap_err()
            .contains("unrelated.txt"));
    }

    #[test]
    fn installed_directory_accepts_quoted_paths_and_skips_stale_entries() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("old MONO.STT");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join(EXE), []).unwrap();

        let quoted = format!("  \"{}\"  ", directory.display());
        assert_eq!(existing_directory([quoted]), Some(directory.clone()));

        let stale = temp.path().join("old path").to_string_lossy().into_owned();
        assert_eq!(
            existing_directory([stale, directory.to_string_lossy().into_owned()]),
            Some(directory)
        );
    }
}

pub fn default_directory() -> Result<PathBuf, String> {
    Ok(
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("找不到本地应用目录")?)
            .join("MONO.STT"),
    )
}

pub fn setup_temp() -> Result<tempfile::TempDir, String> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("mono-stt-setup-");
    if unsafe { IsUserAnAdmin() } != 0 {
        let directory = std::env::var_os("ProgramFiles").ok_or("找不到受保护的程序目录")?;
        return builder.tempdir_in(directory).map_err(|e| e.to_string());
    }
    builder.tempdir().map_err(|e| e.to_string())
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_owned());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_owned());
    left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
}

pub fn target(requested: Option<&Path>, previous: Option<&Installed>) -> Result<PathBuf, String> {
    let directory = match previous {
        Some(info) => info.directory.clone(),
        None => requested
            .map(Path::to_owned)
            .unwrap_or(default_directory()?),
    };
    let local_drive = matches!(
        directory.components().next(),
        Some(std::path::Component::Prefix(prefix))
            if matches!(prefix.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_))
    );
    if !local_drive
        || !directory.is_absolute()
        || directory.parent().is_none()
        || directory
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err("请选择一个完整的本地安装目录。".into());
    }
    if protected_directory(&directory) && unsafe { IsUserAnAdmin() } == 0 {
        return Err("ELEVATION_REQUIRED".into());
    }
    if previous.is_none() && directory.exists() {
        check_fresh_directory(&directory, &default_directory()?)?;
    }
    Ok(directory)
}

fn check_fresh_directory(directory: &Path, default: &Path) -> Result<(), String> {
    let default_location = same_path(directory, default);
    let files = fs::read_dir(directory).map_err(|e| format!("无法读取安装目录：{e}"))?;
    for entry in files {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let known = ["data", "models", MAINTENANCE, APP_ICON]
            .iter()
            .any(|allowed| name == *allowed);
        // The Windows build script keeps the local updater signing material here.
        let signing = default_location
            && name == "Signing"
            && entry.file_type().is_ok_and(|kind| kind.is_dir());
        if !known && !signing {
            return Err(format!(
                "安装目录包含「{}」。请选择另一个安装位置。",
                name.to_string_lossy()
            ));
        }
    }
    Ok(())
}

fn protected_directory(directory: &Path) -> bool {
    [
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "SystemRoot",
    ]
    .iter()
    .filter_map(std::env::var_os)
    .map(PathBuf::from)
    .any(|folder| {
        let folder = folder.canonicalize().unwrap_or(folder);
        directory
            .ancestors()
            .any(|parent| same_path(parent, &folder))
    })
}

fn windows_utility(name: &str) -> Result<Command, String> {
    let system = std::env::var_os("SystemRoot").ok_or("找不到 Windows 系统目录")?;
    let mut command = Command::new(PathBuf::from(system).join("System32").join(name));
    command.creation_flags(0x08000000);
    Ok(command)
}

pub fn current_sid() -> Result<String, String> {
    let output = windows_utility("whoami.exe")?
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let sid = text
        .trim()
        .rsplit_once(',')
        .map(|(_, sid)| sid.trim_matches('"'))
        .unwrap_or("");
    if !output.status.success()
        || !sid.starts_with("S-1-")
        || !sid[4..].bytes().all(|c| c.is_ascii_digit() || c == b'-')
    {
        return Err("无法识别当前 Windows 用户。".into());
    }
    Ok(sid.to_owned())
}

pub fn make_user_storage_writable(directory: &Path) -> Result<(), String> {
    if !protected_directory(directory) {
        return Ok(());
    }
    let grant = format!("*{}:(OI)(CI)M", current_sid()?);
    for name in ["data", "models"] {
        let path = directory.join(name);
        fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("数据或模型目录是重解析点，无法安全调整权限。".into());
        }
        let status = windows_utility("icacls.exe")?
            .arg(path)
            .arg("/grant")
            .arg(&grant)
            .output()
            .map_err(|e| e.to_string())?;
        if !status.status.success() {
            return Err("无法设置记录和模型目录的用户写入权限。".into());
        }
    }
    Ok(())
}

pub fn elevate(package: &Path, directory: Option<&Path>) -> Result<(), String> {
    let sid = current_sid()?;
    let mut arguments = format!("--account {sid}");
    if let Some(directory) = directory {
        let path = directory.to_string_lossy();
        if !directory.is_absolute() || path.contains(['"', '\r', '\n']) {
            return Err("安装目录无效。".into());
        }
        arguments.push_str(&format!(
            " --directory \"{}\"",
            path.trim_end_matches(['\\', '/'])
        ));
    }
    run_elevated(package, &arguments)
}

fn run_elevated(executable: &Path, arguments: &str) -> Result<(), String> {
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            wide("runas").as_ptr(),
            wide(executable).as_ptr(),
            wide(arguments).as_ptr(),
            null(),
            SW_SHOWNORMAL,
        )
    };
    if result as usize <= 32 {
        return Err("管理员授权未完成。".into());
    }
    Ok(())
}

pub struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(self.0) };
        }
    }
}

pub fn install_lock() -> Result<Handle, String> {
    let (handle, exists) = unsafe {
        let handle = CreateMutexW(null(), 0, wide(r"Local\MONO.STT.Installation").as_ptr());
        (Handle(handle), GetLastError() == ERROR_ALREADY_EXISTS)
    };
    if handle.0.is_null() {
        return Err("无法锁定 MONO.STT 安装。".into());
    }
    if exists {
        return Err("另一个 MONO.STT 安装或卸载正在进行。".into());
    }
    Ok(handle)
}

pub fn close_app(directory: &Path) -> Result<(), String> {
    let snapshot = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) });
    if snapshot.0 == INVALID_HANDLE_VALUE {
        return Err(format!(
            "无法检查正在运行的进程：{}",
            io::Error::last_os_error()
        ));
    }
    let executable = directory.join(EXE);
    let worker = directory.join("runtime/python.exe");
    let mut entries = Vec::new();
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut entry_exists = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
    while entry_exists {
        entries.push((
            entry.th32ProcessID,
            entry.th32ParentProcessID,
            String::from_utf16_lossy(&entry.szExeFile),
        ));
        entry_exists = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
    }
    for (pid, _, name) in &entries {
        if !name.trim_end_matches('\0').eq_ignore_ascii_case(EXE) {
            continue;
        }
        let Some(process) = matching_process(*pid, &executable) else {
            continue;
        };
        for (child, parent, _) in &entries {
            if parent == pid {
                if let Some(process) = matching_process(*child, &worker) {
                    terminate(process)?;
                }
            }
        }
        terminate(process)?;
    }
    Ok(())
}

fn matching_process(pid: u32, expected: &Path) -> Option<Handle> {
    let process = Handle(unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    });
    if process.0.is_null() {
        return None;
    }
    let mut path = vec![0u16; 32768];
    let mut len = path.len() as u32;
    (unsafe { QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &mut len) } != 0
        && same_path(
            Path::new(&String::from_utf16_lossy(&path[..len as usize])),
            expected,
        ))
    .then_some(process)
}

fn terminate(process: Handle) -> Result<(), String> {
    let finished = unsafe {
        WaitForSingleObject(process.0, 0) == 0
            || (TerminateProcess(process.0, 1) != 0 && WaitForSingleObject(process.0, 5000) == 0)
    };
    if !finished {
        return Err("无法关闭当前安装目录中的 MONO.STT，请手动退出后重试。".into());
    }
    Ok(())
}

fn shortcut_path() -> Result<PathBuf, String> {
    Ok(
        PathBuf::from(std::env::var_os("APPDATA").ok_or("找不到开始菜单目录")?)
            .join(r"Microsoft\Windows\Start Menu\Programs\MONO.STT.lnk"),
    )
}

pub fn shortcut(directory: &Path, desktop: bool) -> Result<(), String> {
    let link = shortcut_path()?;
    create_shortcut(directory, &link)?;
    if desktop {
        create_shortcut(directory, &desktop_path()?)?;
    } else if let Ok(link) = desktop_path() {
        if link.is_file() && shortcut_points_to(&link, &directory.join(EXE)).unwrap_or(false) {
            create_shortcut(directory, &link)?;
        }
    }
    Ok(())
}

pub fn refresh_shell_icons(directory: &Path) {
    use windows_sys::Win32::UI::Shell::{
        SHChangeNotify, SHCNE_UPDATEITEM, SHCNF_FLUSHNOWAIT, SHCNF_PATHW,
    };

    let executables = [EXE, MAINTENANCE, UNINSTALL_UI].map(|name| directory.join(name));
    for path in executables
        .into_iter()
        .chain(shortcut_path().ok())
        .chain(desktop_path().ok())
        .filter(|path| path.is_file())
    {
        let path = wide(path);
        unsafe {
            SHChangeNotify(
                SHCNE_UPDATEITEM as i32,
                SHCNF_PATHW | SHCNF_FLUSHNOWAIT,
                path.as_ptr().cast(),
                std::ptr::null(),
            );
        }
    }
}

fn desktop_path() -> Result<PathBuf, String> {
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, SHGetKnownFolderPath, KNOWN_FOLDER_FLAG};
    let path = unsafe { SHGetKnownFolderPath(&FOLDERID_Desktop, KNOWN_FOLDER_FLAG(0), None) }
        .map_err(|e| e.to_string())?;
    let text = unsafe { path.to_string() };
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(path.0.cast())) };
    Ok(PathBuf::from(text.map_err(|e| e.to_string())?).join("MONO.STT.lnk"))
}

struct ShortcutApartment;
impl Drop for ShortcutApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}
impl ShortcutApartment {
    fn enter() -> windows::core::Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        Ok(Self)
    }
}

fn create_shortcut(directory: &Path, link: &Path) -> Result<(), String> {
    let _apartment = ShortcutApartment::enter().map_err(|e| e.to_string())?;
    unsafe {
        let shell: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        shell
            .SetPath(PCWSTR(wide(directory.join(EXE)).as_ptr()))
            .map_err(|e| e.to_string())?;
        shell
            .SetWorkingDirectory(PCWSTR(wide(directory).as_ptr()))
            .map_err(|e| e.to_string())?;
        shell
            .SetIconLocation(PCWSTR(wide(directory.join(APP_ICON)).as_ptr()), 0)
            .map_err(|e| e.to_string())?;
        let persist: IPersistFile = shell.cast().map_err(|e| e.to_string())?;
        persist
            .Save(PCWSTR(wide(link).as_ptr()), true)
            .map_err(|e| e.to_string())
    }
}

fn shortcut_points_to(link: &Path, executable: &Path) -> windows::core::Result<bool> {
    let _apartment = ShortcutApartment::enter()?;
    unsafe {
        let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let persist: IPersistFile = shell.cast()?;
        persist.Load(PCWSTR(wide(link).as_ptr()), STGM_READ)?;
        let mut destination = [0u16; 32768];
        shell.GetPath(&mut destination, null_mut(), SLGP_RAWPATH.0 as u32)?;
        let length = destination
            .iter()
            .position(|part| *part == 0)
            .unwrap_or(destination.len());
        Ok(length != 0
            && same_path(
                Path::new(&String::from_utf16_lossy(&destination[..length])),
                executable,
            ))
    }
}

pub fn register(directory: &Path, version: &str) -> Result<(), String> {
    let path = directory.to_string_lossy();
    let command = format!("\"{}\" --uninstall", directory.join(MAINTENANCE).display());
    let quiet = format!("{command} /quiet");
    let icon = directory.join(EXE);
    reg_set(
        UNINSTALL,
        &[
            ("DisplayName", "MONO.STT"),
            ("DisplayVersion", version),
            ("Publisher", "Rui333"),
            ("InstallLocation", &path),
            ("DisplayIcon", &icon.to_string_lossy()),
            ("UninstallString", &command),
            ("QuietUninstallString", &quiet),
        ],
    )?;
    reg_set(LOCATION, &[("", &path)])
}

pub fn check_uninstall(directory: &Path) -> Result<(), String> {
    let registered = reg_get(HKEY_CURRENT_USER, UNINSTALL, "InstallLocation")
        .map(|path| PathBuf::from(path.trim().trim_matches('"')))
        .ok_or("没有检测到已注册的 MONO.STT 安装")?;
    if !registered.is_absolute()
        || !same_path(directory, &registered)
        || !directory.join(MAINTENANCE).is_file()
    {
        return Err("卸载程序所在路径与已注册安装不匹配。".into());
    }
    if protected_directory(directory) && unsafe { IsUserAnAdmin() } == 0 {
        return Err("ELEVATION_REQUIRED".into());
    }
    Ok(())
}

pub fn elevate_uninstall(executable: &Path) -> Result<(), String> {
    run_elevated(
        executable,
        &format!("--uninstall --account {}", current_sid()?),
    )
}

pub fn retired_uninstaller(directory: &Path, launcher: u32) -> PathBuf {
    directory.join(format!(".mono-stt-uninstalled-{launcher}.exe"))
}

pub fn clean_retired_uninstaller(directory: &Path, launcher: u32) -> Result<(), String> {
    let path = retired_uninstaller(directory, launcher);
    windows_utility("WindowsPowerShell/v1.0/powershell.exe")?
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            r#"
            Wait-Process -Id ([int]$env:MONO_UNINSTALL_LAUNCHER) -ErrorAction SilentlyContinue
            $file = $env:MONO_UNINSTALL_RETIRED
            if ([IO.File]::Exists($file)) {
                [IO.File]::Delete($file)
                try { [IO.Directory]::Delete([IO.Path]::GetDirectoryName($file), $false) } catch { }
            }
        "#,
        ])
        .env("MONO_UNINSTALL_LAUNCHER", launcher.to_string())
        .env("MONO_UNINSTALL_RETIRED", path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法启动卸载后清理：{e}"))
}

pub fn uninstall(directory: &Path, launcher: u32) -> Result<(), String> {
    let _lock = install_lock()?;
    check_uninstall(directory)?;
    close_app(directory)?;
    fs::remove_dir_all(directory.join("runtime"))
        .or_else(ignore_missing)
        .map_err(|e| e.to_string())?;
    let executable = directory.join(EXE);
    for shortcut in [Some(shortcut_path()?), desktop_path().ok()]
        .into_iter()
        .flatten()
    {
        if shortcut.is_file() && shortcut_points_to(&shortcut, &executable).unwrap_or(false) {
            fs::remove_file(shortcut)
                .or_else(ignore_missing)
                .map_err(|e| e.to_string())?;
        }
    }
    fs::remove_file(executable)
        .or_else(ignore_missing)
        .map_err(|e| e.to_string())?;
    let maintenance = directory.join(MAINTENANCE);
    let retired = retired_uninstaller(directory, launcher);
    fs::rename(&maintenance, &retired).map_err(|e| format!("无法移除卸载程序：{e}"))?;
    if let Err(error) = remove_registration() {
        let _ = fs::rename(&retired, &maintenance);
        return Err(error);
    }
    let _ = fs::remove_file(directory.join(UNINSTALL_UI));
    let _ = fs::remove_file(directory.join(APP_ICON));
    Ok(())
}

fn remove_registration() -> Result<(), String> {
    for key in [LOCATION, UNINSTALL] {
        let result = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(key).as_ptr()) };
        if result != 0 && result != ERROR_FILE_NOT_FOUND {
            return Err(io::Error::from_raw_os_error(result as i32).to_string());
        }
    }
    Ok(())
}

fn ignore_missing(error: io::Error) -> io::Result<()> {
    if error.kind() == io::ErrorKind::NotFound {
        Ok(())
    } else {
        Err(error)
    }
}

pub fn message(text: &str, flags: u32) -> i32 {
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(text).as_ptr(),
            wide("MONO.STT 安装").as_ptr(),
            flags,
        )
    }
}
