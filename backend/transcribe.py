"""Convert the first audio stream to a seekable WAV and emit JSON lines."""
import json
import os
import ctypes
from pathlib import Path
import site
import sys
import wave


def emit(kind, **fields):
    print(json.dumps({"kind": kind, **fields}, ensure_ascii=True), flush=True)


def configure_dll_search():
    handles = []
    if os.name == "nt":
        roots = site.getsitepackages()
        local = os.environ.get("LOCALAPPDATA")
        # Reuse the existing MONO.STT runtime when the installer omits CUDA DLLs.
        if local:
            roots.append(str(Path(local) / "InkVoice" / "runtime" / "Lib" / "site-packages"))
        for root in dict.fromkeys(roots):
            for folder in Path(root).glob("nvidia/*/bin"):
                os.environ["PATH"] = str(folder) + os.pathsep + os.environ["PATH"]
                handles.append(os.add_dll_directory(str(folder)))
        program_files = os.environ.get("ProgramFiles")
        if program_files:
            toolkit = Path(program_files) / "NVIDIA GPU Computing Toolkit" / "CUDA"
            for folder in sorted(toolkit.glob("v12.*/bin"), reverse=True):
                os.environ["PATH"] = str(folder) + os.pathsep + os.environ["PATH"]
                handles.append(os.add_dll_directory(str(folder)))
    return handles


def cuda_modes():
    import ctranslate2

    preference = ("int8_float16", "int8_float32", "float16", "float32")
    devices = []
    for index in range(ctranslate2.get_cuda_device_count()):
        supported = ctranslate2.get_supported_compute_types("cuda", index)
        mode = next((mode for mode in preference if mode in supported), None)
        if mode:
            devices.append((index, mode))
    if not devices:
        raise RuntimeError("推理引擎未找到可用的 CUDA 设备")
    if os.name == "nt":
        try:
            ctypes.WinDLL("cublas64_12.dll", winmode=0x00001000)
        except OSError:
            # Also accept libraries installed system-wide or found on PATH.
            try:
                ctypes.WinDLL("cublas64_12.dll", winmode=0)
            except OSError as error:
                raise RuntimeError("缺少 CUDA 12 cuBLAS 运行库（cublas64_12.dll）") from error
    return devices


def probe_cuda():
    try:
        devices = cuda_modes()
        return {"cudaAvailable": True, "computeTypes": sorted({mode for _, mode in devices})}
    except (ImportError, OSError, RuntimeError, ValueError) as error:
        return {"cudaAvailable": False, "reason": str(error)}


def recognize(audio, model_dir, device, duration, options, compute_type="int8", device_index=0):
    from faster_whisper import WhisperModel
    label = f"CUDA · {compute_type}" if device == "cuda" else "CPU"
    emit("status", message=f"加载模型 · {label}")
    model = WhisperModel(str(model_dir), device=device, device_index=device_index,
                         compute_type=compute_type,
                         local_files_only=True, cpu_threads=int(options.get("cpuThreads", 8)))
    segments, info = model.transcribe(str(audio), beam_size=3, vad_filter=options.get("vadFilter", True),
                                      language=options.get("language") if options.get("language") in {"zh","en","ja"} else None)
    result = []
    last_progress = -1
    for segment in segments:
        text = segment.text.strip()
        if text:
            result.append(dict(start=segment.start, end=segment.end, text=text))
        progress = min(99, int(100 * segment.end / duration))
        if progress > last_progress:
            emit("status", message=f"正在转写 · {label} · {progress}%")
            last_progress = progress
    return dict(segments=result, duration=duration, language=info.language, device=device,
                computeType=compute_type)


def extract_audio(source, output):
    import av

    try:
        with av.open(str(source)) as media:
            if not media.streams.audio:
                raise ValueError("文件没有音轨")
            resampler = av.AudioResampler(format="s16", layout="mono", rate=16000)
            with wave.open(str(output), "wb") as wav:
                wav.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
                for frame in media.decode(audio=0):
                    for sample in resampler.resample(frame):
                        wav.writeframesraw(sample.to_ndarray().tobytes())
                for sample in resampler.resample(None):
                    wav.writeframesraw(sample.to_ndarray().tobytes())
                return wav.getnframes() / 16000
    except av.FFmpegError as error:
        raise ValueError(f"音频解码失败：{error}") from error


def main():
    options = json.loads(sys.argv[4]) if len(sys.argv)>4 else {}
    source, folder, output = map(Path, sys.argv[1:4])
    if not (folder / "model.bin").is_file():
        raise ValueError(f"找不到模型文件：{folder}")
    emit("status", message="正在提取音轨")
    duration = extract_audio(source, output)
    if duration <= 0:
        raise ValueError("音轨为空")
    # Sample equal time intervals without loading the whole recording.
    import numpy as np
    with wave.open(str(output),"rb") as wav:
        frames=wav.getnframes()
        peaks=[]
        for i in range(72):
            wav.setpos(i*frames//72)
            samples=np.frombuffer(wav.readframes(max(1,min(2048,frames//72))),dtype="<i2")
            peaks.append(float(np.abs(samples.astype(float)).max()) if samples.size else 0)
    scale=max(max(peaks),1)
    metadata=dict(fileSizeBytes=source.stat().st_size,waveform=[max(.08,p/scale) for p in peaks])
    device = options.get("computeDevice","auto")
    if device in ("auto", "cuda"):
        try:
            devices = cuda_modes()
            for index, mode in devices:
                try:
                    return dict(recognize(output, folder, "cuda", duration, options, mode, index), **metadata)
                except (RuntimeError, ValueError) as error:
                    last_failure = str(error)
            raise RuntimeError(last_failure)
        except (RuntimeError, ValueError) as err:
            if device == "cuda":
                raise RuntimeError(f"CUDA 转写失败：{err}") from err
            emit("status", message=f"CUDA 转写失败，已切换到 CPU：{err}")
    return dict(recognize(output, folder, "cpu", duration, options), **metadata)


if __name__ == "__main__":
    handles = configure_dll_search()
    try:
        emit("result", **(probe_cuda() if sys.argv[1:] == ["--probe-cuda"] else main()))
    except Exception as exc:
        emit("error", message=str(exc))
        sys.exit(1)
