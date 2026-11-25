use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{ipc::Channel, AppHandle, Manager};
use tauri_plugin_updater::UpdaterExt;

#[derive(Default)]
pub struct Activity(Arc<Mutex<ActiveWork>>);

#[derive(Default)]
struct ActiveWork {
    tasks: usize,
    updating: bool,
}

pub struct Work(Arc<Mutex<ActiveWork>>, bool);

impl Drop for Work {
    fn drop(&mut self) {
        if let Ok(mut active) = self.0.lock() {
            if self.1 {
                active.updating = false;
            } else {
                active.tasks -= 1;
            }
        }
    }
}

impl Activity {
    fn enter(&self, update: bool) -> Result<Work, String> {
        let mut active = self.0.lock().map_err(|_| "无法读取应用任务状态")?;
        if active.updating {
            return Err("正在安装更新，请稍后重试。".into());
        }
        if update {
            if active.tasks > 0 {
                return Err("请等录音、转录或模型下载结束，再安装更新。".into());
            }
            active.updating = true;
        } else {
            active.tasks += 1;
        }
        Ok(Work(self.0.clone(), update))
    }
}

pub fn task(app: &AppHandle) -> Result<Work, String> {
    app.state::<Activity>().enter(false)
}

fn enabled() -> bool {
    option_env!("MONO_SIGNED_UPDATE_BUILD") == Some("1")
}

#[tauri::command]
pub fn app_update_info(app: AppHandle) -> Value {
    json!({"version": app.package_info().version.to_string(), "enabled": enabled()})
}

#[tauri::command]
pub async fn check_app_update(app: AppHandle) -> Result<Option<Value>, String> {
    if !enabled() {
        return Err("当前构建未启用在线更新。".into());
    }
    let update = app
        .updater_builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(update_error)?
        .check()
        .await
        .map_err(update_error)?;
    Ok(update.map(|update| json!({"version": update.version, "notes": update.body})))
}

#[tauri::command]
pub async fn install_app_update(
    app: AppHandle,
    version: String,
    progress: Channel<Value>,
) -> Result<(), String> {
    if !enabled() {
        return Err("当前构建未启用在线更新。".into());
    }
    let _install = app.state::<Activity>().enter(true)?;
    let update = app
        .updater_builder()
        .timeout(Duration::from_secs(900))
        .build()
        .map_err(update_error)?
        .check()
        .await
        .map_err(update_error)?
        .filter(|update| update.version == version)
        .ok_or("可用版本已变化，请重新检查更新。")?;
    let mut received = 0u64;
    let mut last_percent = None;
    update
        .download_and_install(
            |chunk, total| {
                received += chunk as u64;
                let percent = total
                    .filter(|&size| size > 0)
                    .map(|size| (received.saturating_mul(100) / size).min(100));
                if received == chunk as u64 || percent != last_percent {
                    let _ = progress.send(json!({"downloaded": received, "total": total}));
                    last_percent = percent;
                }
            },
            || {
                let _ = progress.send(json!({"installing": true}));
            },
        )
        .await
        .map_err(update_error)
}

fn update_error(error: tauri_plugin_updater::Error) -> String {
    format!("软件更新失败：{error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_and_background_work_are_mutually_exclusive() {
        let state = Activity::default();
        let first = state.enter(false).unwrap();
        let second = state.enter(false).unwrap();
        assert!(state.enter(true).is_err());
        drop(first);
        assert!(state.enter(true).is_err());
        drop(second);
        let update = state.enter(true).unwrap();
        assert!(state.enter(false).is_err());
        assert!(state.enter(true).is_err());
        drop(update);
        assert!(state.enter(false).is_ok());
    }
}
