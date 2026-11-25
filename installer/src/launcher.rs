#![windows_subsystem = "windows"]

use mono_stt_installer::{package, system};

use std::{
    env,
    path::Path,
    process::{self, Command},
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONERROR, MB_ICONQUESTION, MB_OK, MB_YESNO,
};

fn run() -> Result<i32, String> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let flag = |name: &str| {
        args.iter()
            .any(|arg| arg.to_string_lossy().eq_ignore_ascii_case(name))
    };
    let setup = env::current_exe().map_err(|e| e.to_string())?;
    if let Some(expected) = args
        .windows(2)
        .find(|pair| pair[0] == "--account")
        .map(|pair| &pair[1])
    {
        if expected.to_string_lossy() != system::current_sid()? {
            return Err("请使用同一个 Windows 账号授权，确保应用和数据仍属于当前用户。".into());
        }
    }
    if flag("--uninstall") {
        if !flag("/s")
            && system::message(
                "卸载 MONO.STT？已有转录记录和模型会保留。",
                MB_YESNO | MB_ICONQUESTION,
            ) != IDYES
        {
            return Ok(1);
        }
        system::uninstall(setup.parent().ok_or("无法定位安装目录")?)?;
        return Ok(0);
    }
    let silent = flag("/s") || flag("/quiet");
    let (mut package_file, parts) = package::open(&setup).map_err(|e| e.to_string())?;
    let temp = system::setup_temp()?;

    if !silent && !system::webview_installed() {
        if system::message(
            "需要安装 Microsoft Edge WebView2 运行组件。是否现在安装？",
            MB_YESNO | MB_ICONQUESTION,
        ) != IDYES
        {
            return Ok(1);
        }
        install_webview(temp.path(), &mut package_file, parts.webview, false)?;
    } else if silent && !system::webview_installed() {
        install_webview(temp.path(), &mut package_file, parts.webview, true)?;
    }

    let gui = temp.path().join("mono-stt-installer.exe");
    package::copy(&mut package_file, parts.gui, &gui).map_err(|e| e.to_string())?;
    let mut command = Command::new(gui);
    command.arg("--package").arg(&setup).args(&args);
    let status = command
        .status()
        .map_err(|e| format!("无法打开安装程序：{e}"))?;
    if !status.success() && !silent {
        return Err(format!(
            "安装程序提前退出（{}）。请重新下载安装包。",
            status
        ));
    }
    Ok(status.code().unwrap_or(1))
}

fn install_webview(
    temp: &Path,
    package_file: &mut std::fs::File,
    part: package::Part,
    silent: bool,
) -> Result<(), String> {
    let path = temp.join("WebView2Setup.exe");
    package::copy(package_file, part, &path).map_err(|e| e.to_string())?;
    let mut command = Command::new(&path);
    if silent {
        command.arg("/silent");
    }
    let status = command
        .arg("/install")
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() || !system::webview_installed() {
        return Err("无法安装 WebView2。请检查网络和 Windows 应用安装权限后重试。".into());
    }
    Ok(())
}

fn main() {
    let silent = env::args_os().any(|arg| {
        ["/s", "/quiet"]
            .iter()
            .any(|flag| arg.to_string_lossy().eq_ignore_ascii_case(flag))
    });
    match run() {
        Ok(code) => process::exit(code),
        Err(error) => {
            if !silent {
                system::message(&error, MB_OK | MB_ICONERROR);
            }
            process::exit(1);
        }
    }
}
