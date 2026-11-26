use std::{ffi::OsStr, path::PathBuf, process::Command};

pub fn command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}

pub fn curl() -> Result<PathBuf, String> {
    #[cfg(windows)]
    return Ok(
        PathBuf::from(std::env::var_os("SystemRoot").ok_or("缺少 SystemRoot")?)
            .join(r"System32\curl.exe"),
    );
    #[cfg(not(windows))]
    return Ok(PathBuf::from("/usr/bin/curl"));
}

pub fn data_root() -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    return Ok(
        PathBuf::from(std::env::var_os("HOME").ok_or("无法找到用户目录")?)
            .join("Library/Application Support/MONO.STT"),
    );
    #[cfg(not(target_os = "macos"))]
    return std::env::current_exe()
        .map_err(|error| error.to_string())?
        .parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| "找不到应用目录".into());
}

pub fn open_url(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
        let url: Vec<u16> = url.encode_utf16().chain([0]).collect();
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
        if result as isize <= 32 {
            return Err("无法打开链接".into());
        }
    }
    #[cfg(not(windows))]
    {
        let status = command("/usr/bin/open")
            .arg(url)
            .status()
            .map_err(|error| error.to_string())?;
        if !status.success() {
            return Err("无法打开链接".into());
        }
    }
    Ok(())
}
