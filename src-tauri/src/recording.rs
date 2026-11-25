use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, SampleFormat, SizedSample,
};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    thread,
};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

type Writer = hound::WavWriter<BufWriter<File>>;

#[derive(Default)]
pub struct Recorder(Mutex<Option<Session>>);

struct Session {
    id: Uuid,
    stop: mpsc::Sender<bool>,
    result: mpsc::Receiver<Result<f64, String>>,
    capture: Arc<Mutex<Capture>>,
}

#[derive(Default)]
struct Capture {
    writer: Option<Writer>,
    peak: f32,
    error: Option<String>,
}

fn recording_path(app: &AppHandle, id: Uuid) -> Result<PathBuf, String> {
    let store = app
        .try_state::<crate::store::Store>()
        .ok_or("录音目录尚未初始化")?;
    Ok(store
        .data_dir()
        .join("recordings")
        .join(format!("{id}.wav")))
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    capture: Arc<Mutex<Capture>>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample,
    i16: FromSample<T>,
{
    let error_target = capture.clone();
    device.build_input_stream(
        config,
        move |samples: &[T], _| {
            let Ok(mut capture) = capture.lock() else {
                return;
            };
            if capture.error.is_some() {
                return;
            }
            let mut peak = 0.0f32;
            if let Some(writer) = capture.writer.as_mut() {
                for &sample in samples {
                    let sample = sample.to_sample::<i16>();
                    peak = peak.max((f32::from(sample) / 32768.0).abs());
                    if let Err(error) = writer.write_sample(sample) {
                        capture.error = Some(format!("无法写入录音：{error}"));
                        break;
                    }
                }
            }
            capture.peak = capture.peak.max(peak);
        },
        move |error| {
            if let Ok(mut capture) = error_target.lock() {
                capture.error = Some(format!("麦克风采集已中断：{error}"));
            }
        },
        None,
    )
}

fn open_stream(path: &Path, capture: &Arc<Mutex<Capture>>) -> Result<(cpal::Stream, u32), String> {
    let device = cpal::default_host()
        .default_input_device()
        .ok_or("没有找到麦克风。请检查接入设备和 Windows 麦克风隐私设置。")?;
    let supported = device
        .default_input_config()
        .map_err(|e| format!("无法读取麦克风设置：{e}"))?;
    let config = supported.config();
    let sample_rate = config.sample_rate.0;
    let writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: config.channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .map_err(|e| format!("无法创建录音文件：{e}"))?;
    capture.lock().map_err(|e| e.to_string())?.writer = Some(writer);
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build_stream::<f32>(&device, &config, capture.clone()),
        SampleFormat::I16 => build_stream::<i16>(&device, &config, capture.clone()),
        SampleFormat::I32 => build_stream::<i32>(&device, &config, capture.clone()),
        SampleFormat::U16 => build_stream::<u16>(&device, &config, capture.clone()),
        format => return Err(format!("麦克风采样格式暂不支持：{format:?}")),
    }
    .map_err(|e| format!("无法打开麦克风，请检查 Windows 麦克风隐私设置：{e}"))?;
    stream
        .play()
        .map_err(|e| format!("无法开始录音，请检查 Windows 麦克风隐私设置：{e}"))?;
    Ok((stream, sample_rate))
}

#[tauri::command]
pub async fn start_native_recording(app: AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let task = crate::updates::task(&app)?;
        let state = app.state::<Recorder>();
        let mut slot = state.0.lock().map_err(|e| e.to_string())?;
        if slot.is_some() {
            return Err("已有录音正在进行".into());
        }
        let id = Uuid::new_v4();
        let path = recording_path(&app, id)?;
        fs::create_dir_all(path.parent().expect("recording has a parent"))
            .map_err(|e| e.to_string())?;
        let capture = Arc::new(Mutex::new(Capture::default()));
        let worker_capture = capture.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (stop, stop_rx) = mpsc::channel();
        let (result_tx, result) = mpsc::channel();

        thread::Builder::new()
            .name("mono-stt-microphone".into())
            .spawn(move || {
                let _task = task;
                let (stream, sample_rate) = match open_stream(&path, &worker_capture) {
                    Ok(value) => value,
                    Err(error) => {
                        if let Ok(mut capture) = worker_capture.lock() {
                            drop(capture.writer.take());
                        }
                        let _ = fs::remove_file(&path);
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                let keep = ready_tx.send(Ok(())).is_ok() && stop_rx.recv() == Ok(true);
                drop(stream);
                let result = (|| {
                    let mut capture = worker_capture.lock().map_err(|e| e.to_string())?;
                    let writer = capture.writer.take().ok_or("录音文件未打开")?;
                    if !keep {
                        return Ok(0.0);
                    }
                    if let Some(error) = capture.error.take() {
                        return Err(error);
                    }
                    let duration = f64::from(writer.duration()) / f64::from(sample_rate);
                    writer
                        .finalize()
                        .map_err(|e| format!("无法保存录音：{e}"))?;
                    if duration <= 0.0 {
                        return Err("未采集到录音，请检查麦克风".into());
                    }
                    Ok(duration)
                })();
                if !keep || result.is_err() {
                    let _ = fs::remove_file(&path);
                }
                let _ = result_tx.send(result);
            })
            .map_err(|e| e.to_string())?;

        ready_rx
            .recv()
            .map_err(|_| "录音初始化意外结束".to_string())??;
        *slot = Some(Session {
            id,
            stop,
            result,
            capture,
        });
        Ok(id.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn native_recording_level(app: AppHandle, id: String) -> Result<f32, String> {
    let state = app.state::<Recorder>();
    let slot = state.0.lock().map_err(|e| e.to_string())?;
    let id = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let session = slot
        .as_ref()
        .filter(|session| session.id == id)
        .ok_or("录音已结束")?;
    let mut capture = session.capture.lock().map_err(|e| e.to_string())?;
    if let Some(error) = &capture.error {
        return Err(error.clone());
    }
    Ok(std::mem::take(&mut capture.peak))
}

#[tauri::command]
pub async fn stop_native_recording(
    app: AppHandle,
    id: String,
    keep: bool,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let id = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
        let state = app.state::<Recorder>();
        let mut slot = state.0.lock().map_err(|e| e.to_string())?;
        if !slot.as_ref().is_some_and(|session| session.id == id) {
            return Err("录音已结束".into());
        }
        let session = slot.take().unwrap();
        let _ = session.stop.send(keep);
        let duration = session
            .result
            .recv()
            .map_err(|_| "录音意外结束".to_string())??;
        if !keep {
            return Ok(Value::Null);
        }
        let path = recording_path(&app, id)?;
        if let Err(error) = app.asset_protocol_scope().allow_file(&path) {
            let _ = fs::remove_file(&path);
            return Err(error.to_string());
        }
        Ok(json!({ "path": path, "duration": duration }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn discard_native_recording(app: AppHandle, id: String) -> Result<(), String> {
    let id = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let state = app.state::<Recorder>();
    let slot = state.0.lock().map_err(|e| e.to_string())?;
    if slot.as_ref().is_some_and(|session| session.id == id) {
        return Err("请先结束录音".into());
    }
    match fs::remove_file(recording_path(&app, id)?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法舍弃录音：{error}")),
    }
}
