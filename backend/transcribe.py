"""Convert the first audio stream to a seekable WAV and emit JSON lines."""
import errno
import shutil
import json
import os
import ctypes
from math import isfinite
import re
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


def probe_runtime():
    import av
    import ctranslate2
    import numpy
    import onnxruntime
    import faster_whisper

    return {"av": av.__version__, "ctranslate2": ctranslate2.__version__,
            "numpy": numpy.__version__, "onnxruntime": onnxruntime.__version__,
            "fasterWhisper": faster_whisper.__version__}


def transcription_terms(options):
    replacements = {}
    hints = {}
    for term in options.get("terms") or []:
        original = term.get("from", "").strip()
        corrected = term.get("to", "").strip()
        if corrected:
            hints[corrected] = None
            if original and original != corrected:
                replacements[original] = corrected
    originals = sorted(replacements, key=len, reverse=True)
    pattern = re.compile("|".join(map(re.escape, originals))) if originals else None
    return ", ".join(hints) or None, pattern, replacements


SAMPLE_RATE = 16000
DISK_RESERVE = 128 * 1024 * 1024
MAX_WAV_DATA = (1 << 32) - 64
RESUME_OVERLAP = 12


def trim_overlap(segment, offset, boundary, previous):
    """Align a segment that straddles the last committed audio timestamp."""
    start, end = segment.start + offset, segment.end + offset
    if end <= boundary:
        return None
    if start >= boundary:
        return start, end, segment.text.strip()
    words = segment.words or []
    normalize = lambda text: "".join(char.casefold() for char in text if char.isalnum())
    suffix = normalize(previous)
    prefix = ""
    matched = 0
    for index, word in enumerate(words):
        if word.end + offset > boundary + 0.6:
            break
        prefix += normalize(word.word)
        if len(prefix) >= 2 and suffix.endswith(prefix):
            matched = index + 1
    if matched:
        words = words[matched:]
    elif words:
        words = [word for word in words if (word.start + word.end) / 2 + offset >= boundary]
    if not words:
        return None
    return max(boundary, words[0].start + offset), end, "".join(word.word for word in words).strip()


class ResumeInterrupted(RuntimeError):
    pass


def recognize(audio, model_dir, device, duration, options, compute_type="int8", device_index=0):
    from faster_whisper import WhisperModel
    label = f"CUDA · {compute_type}" if device == "cuda" else "CPU"
    emit("reset")
    boundary = options.get("_resumeAt", 0)
    if boundary:
        emit("status", message=f"从检查点继续 · {label} · {min(99, int(100 * boundary / duration))}%")
    else:
        emit("status", message=f"加载模型 · {label}")
    model = WhisperModel(str(model_dir), device=device, device_index=device_index,
                         compute_type=compute_type,
                         local_files_only=True, cpu_threads=int(options.get("cpuThreads", 8)))
    hotwords, correction_pattern, replacements = transcription_terms(options)
    offset = 0.0
    model_audio = str(audio)
    if boundary > 0:
        import numpy as np
        with wave.open(str(audio), "rb") as wav:
            frame = max(0, int((boundary - RESUME_OVERLAP) * SAMPLE_RATE))
            wav.setpos(frame)
            offset = frame / SAMPLE_RATE
            samples = np.frombuffer(wav.readframes(wav.getnframes() - frame), dtype="<i2")
        model_audio = samples.astype(np.float32)
        del samples
        model_audio /= 32768.0
    requested_language = options.get("language")
    language = options.get("_resumeLanguage") or (requested_language if requested_language in {"zh", "en", "ja"} else None)
    segments, info = model.transcribe(model_audio, beam_size=3, vad_filter=options.get("vadFilter", True),
                                      hotwords=hotwords, language=language,
                                      word_timestamps=boundary > 0)
    emit("language", language=info.language)
    result = None if options.get("_streamSegmentsOnly") else []
    last_progress = -1
    emitted = False
    try:
        for segment in segments:
            if not isfinite(segment.start) or not isfinite(segment.end):
                continue
            if boundary and segment.start + offset < boundary:
                row = trim_overlap(segment, offset, boundary, options.get("_resumeText", ""))
                if row is None:
                    continue
                start, end, text = row
            else:
                start, end, text = segment.start + offset, segment.end + offset, segment.text.strip()
            start, end = max(0.0, start), min(duration, end)
            if start >= end:
                continue
            if text:
                if correction_pattern:
                    text = correction_pattern.sub(lambda match: replacements[match.group()], text)
                row = dict(start=start, end=end, text=text)
                if result is not None:
                    result.append(row)
                emit("segment", **row)
                emitted = True
            progress = min(99, int(100 * end / duration))
            if progress > last_progress:
                emit("status", message=f"正在转写 · {label} · {progress}%")
                last_progress = progress
    except (RuntimeError, ValueError) as error:
        if boundary and emitted:
            raise ResumeInterrupted(str(error)) from error
        raise
    summary = dict(duration=duration, language=info.language, device=device, computeType=compute_type)
    if result is not None:
        summary["segments"] = result
    return summary


def extract_audio(source, output):
    import av

    free = shutil.disk_usage(output.parent).free
    if free < DISK_RESERVE + 1024 * 1024:
        raise ValueError("保存音轨的磁盘空间不足，请释放空间后重试")
    written = 0
    checked = 0
    try:
        with av.open(str(source)) as media:
            if not media.streams.audio:
                raise ValueError("文件没有音轨")
            resampler = av.AudioResampler(format="s16", layout="mono", rate=SAMPLE_RATE)
            with wave.open(str(output), "wb") as wav:
                wav.setparams((1, 2, SAMPLE_RATE, 0, "NONE", "not compressed"))

                def write_samples(samples):
                    nonlocal written, checked
                    for sample in samples:
                        data = sample.to_ndarray().tobytes()
                        if written + len(data) > MAX_WAV_DATA:
                            raise ValueError("音轨超出 WAV 大小上限，请拆分文件后重试")
                        if written - checked + len(data) >= 1024 * 1024:
                            if shutil.disk_usage(output.parent).free < DISK_RESERVE + len(data):
                                raise ValueError("提取音轨时磁盘空间不足，请释放空间后重试")
                            checked = written
                        wav.writeframesraw(data)
                        written += len(data)

                for frame in media.decode(audio=0):
                    write_samples(resampler.resample(frame))
                write_samples(resampler.resample(None))
                return wav.getnframes() / SAMPLE_RATE
    except av.FFmpegError as error:
        raise ValueError(f"音频解码失败：{error}") from error
    except OSError as error:
        if error.errno == errno.ENOSPC:
            raise ValueError("提取音轨时磁盘空间不足，请释放空间后重试") from error
        raise


def main():
    options = json.loads(sys.argv[4]) if len(sys.argv)>4 else {}
    source, folder, output = map(Path, sys.argv[1:4])
    if not (folder / "model.bin").is_file():
        raise ValueError(f"找不到模型文件：{folder}")
    retained = options.get("_retainedAudio")
    if options.get("_reuseAudio"):
        output = Path(retained)
        emit("status", message="正在读取已保存音轨")
        with wave.open(str(output), "rb") as wav:
            if (wav.getnchannels(), wav.getsampwidth(), wav.getframerate()) != (1, 2, SAMPLE_RATE):
                raise ValueError("已保存音轨格式不匹配，请从头转录")
            duration = wav.getnframes() / SAMPLE_RATE
        file_size = options["_sourceSize"]
    else:
        emit("status", message="正在提取音轨")
        duration = extract_audio(source, output)
        if duration <= 0:
            raise ValueError("音轨为空")
        file_size = source.stat().st_size
        if retained:
            target = Path(retained)
            os.replace(output, target)
            output = target
    if duration <= 0 or options.get("_resumeAt", 0) > duration:
        raise ValueError("已保存音轨无效，请从头转录")
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
    metadata=dict(fileSizeBytes=file_size,waveform=[max(.08,p/scale) for p in peaks])
    emit("metadata", duration=duration, **metadata)
    device = options.get("computeDevice","auto")
    if os.name == "nt" and device in ("auto", "cuda"):
        try:
            devices = cuda_modes()
            for index, mode in devices:
                try:
                    return dict(recognize(output, folder, "cuda", duration, options, mode, index), **metadata)
                except ResumeInterrupted:
                    raise
                except (RuntimeError, ValueError) as error:
                    last_failure = str(error)
            raise RuntimeError(last_failure)
        except ResumeInterrupted:
            raise
        except (RuntimeError, ValueError) as err:
            if device == "cuda":
                raise RuntimeError(f"CUDA 转写失败：{err}") from err
            emit("status", message=f"CUDA 转写失败，已切换到 CPU：{err}")
    return dict(recognize(output, folder, "cpu", duration, options), **metadata)


if __name__ == "__main__":
    handles = configure_dll_search()
    try:
        if sys.argv[1:] == ["--probe-runtime"]:
            emit("result", **probe_runtime())
        else:
            emit("result", **(probe_cuda() if sys.argv[1:] == ["--probe-cuda"] else main()))
    except Exception as exc:
        emit("error", message=str(exc))
        sys.exit(1)
