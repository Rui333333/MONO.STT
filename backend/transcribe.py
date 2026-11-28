"""Convert the first audio stream to a seekable WAV and emit JSON lines."""
import errno
import hashlib
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

    from opencc import OpenCC
    if sys.platform == "darwin":
        import mlx.core as mx
        # Hosted macOS runners can package the runtime without a Metal device.
        metal_available = mx.metal.is_available()
    else:
        metal_available = False
    if OpenCC("t2s").convert("繁體") != "繁体":
        raise RuntimeError("繁简转换字典不可用")
    return {"av": av.__version__, "ctranslate2": ctranslate2.__version__,
            "numpy": numpy.__version__, "onnxruntime": onnxruntime.__version__,
            "fasterWhisper": faster_whisper.__version__, "metalAvailable": metal_available}


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


def trim_overlap(segment, offset, boundary, previous, display_text=str):
    """Align a segment that straddles the last committed audio timestamp."""
    start, end = segment.start + offset, segment.end + offset
    if end <= boundary:
        return None
    if start >= boundary:
        return start, end, segment.text.strip(), segment.words or []
    words = segment.words or []
    normalize = lambda text: "".join(char.casefold() for char in text if char.isalnum())
    suffix = normalize(previous)
    prefix = ""
    matched = 0
    for index, word in enumerate(words):
        if word.end + offset > boundary + 0.6:
            break
        prefix += word.word
        candidate = normalize(display_text(prefix))
        if len(candidate) >= 2 and suffix.endswith(candidate):
            matched = index + 1
    if matched:
        words = words[matched:]
    elif words:
        words = [word for word in words if (word.start + word.end) / 2 + offset >= boundary]
    if not words:
        return None
    return max(boundary, words[0].start + offset), end, "".join(word.word for word in words).strip(), words


def timed_words(words, offset, start, end, text, convert):
    items = []
    for word in words:
        begin = max(start, word.start + offset)
        finish = min(end, max(begin, word.end + offset))
        if begin <= finish and isfinite(begin) and isfinite(finish):
            items.append(dict(text=convert(word.word), startSec=begin, endSec=finish))
    if not items:
        return []
    items[0]["text"] = items[0]["text"].lstrip()
    items[-1]["text"] = items[-1]["text"].rstrip()
    return items if "".join(item["text"] for item in items) == text else []


def correct_timed_words(text, words, pattern, replacements):
    if pattern is None:
        return text, words
    if not words:
        return pattern.sub(lambda match: replacements[match.group()], text), words
    matches = list(pattern.finditer(text))
    if not matches:
        return text, words

    # Source offsets refer to the original display text, even when replacements change its length.
    # Each interval is consumed once, preserving intact model words verbatim.
    source = iter(word for word in words if word["text"])
    word = next(source, None)
    word_start = 0
    position = 0
    corrected = []

    def consume(stop, preserve):
        nonlocal position, word, word_start
        first_time = last_time = None
        while position < stop:
            size = len(word["text"])
            word_end = word_start + size
            portion_end = min(stop, word_end)
            if preserve and position == word_start and portion_end == word_end:
                corrected.append(word)
            else:
                start, end = word["startSec"], word["endSec"]
                scale = (end - start) / size
                begin = start if position == word_start else start + (position - word_start) * scale
                finish = end if portion_end == word_end else start + (portion_end - word_start) * scale
                if first_time is None:
                    first_time = begin
                last_time = finish
                if preserve:
                    corrected.append(dict(text=text[position:portion_end], startSec=begin,
                                          endSec=finish, estimated=True))
            position = portion_end
            if position == word_end:
                word = next(source, None)
                word_start = word_end
        return first_time, last_time

    for match in matches:
        consume(match.start(), preserve=True)
        begin, finish = consume(match.end(), preserve=False)
        corrected.append(dict(text=replacements[match.group()], startSec=begin,
                              endSec=max(begin, finish), estimated=True))
    consume(len(text), preserve=True)
    return "".join(word["text"] for word in corrected), corrected


def verify_model_file(folder, name, size, digest):
    path = folder / name
    if not path.is_file():
        raise ValueError(f"缺少模型文件：{path}")
    if path.stat().st_size != size:
        raise ValueError(f"模型文件大小不匹配：{path}")
    with path.open("rb") as file:
        if hashlib.file_digest(file, "sha256").hexdigest() != digest:
            raise ValueError(f"模型文件 SHA-256 校验失败：{path}")


MLX_MODELS = {
    "faster-whisper-large-v3-turbo": (
        (268, "b34fc29e4e11e0a25e812775dd67f4dd16fc2c8eb43d28ae25ff7d660ecb6379"),
        ("weights.safetensors", 1613977612, "951ed3fc1203e6a62467abb2144a96ce7eafca8fa77e3704fdb8635ff3e7f8a6"),
    ),
    "faster-whisper-large-v3": (
        (269, "34982ce6ae286095000f82ae9583b3431639e8b092bf60c961f203745e6500e3"),
        ("weights.npz", 3083520416, "05ff791ce3630fae47e7c51004e9666204d786246ec07cac6110af768099b40d"),
    ),
}


class ResumeInterrupted(RuntimeError):
    pass


def recognize(audio, model_dir, device, duration, options, compute_type="int8", device_index=0):
    label = f"CUDA · {compute_type}" if device == "cuda" else ("Apple Metal" if device == "metal" else "CPU")
    emit("reset")
    boundary = options.get("_resumeAt", 0)
    if boundary:
        emit("status", message=f"从检查点继续 · {label} · {min(99, int(100 * boundary / duration))}%")
    else:
        emit("status", message=f"加载模型 · {label}")
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
    prompt = "以下是普通话录音的简体中文逐字稿，保留口语内容并使用中文标点。" if language == "zh" else None
    if device == "metal":
        import mlx_whisper
        import numpy as np
        from types import SimpleNamespace

        if isinstance(model_audio, str):
            with wave.open(model_audio, "rb") as wav:
                model_audio = np.frombuffer(wav.readframes(wav.getnframes()), dtype="<i2").astype(np.float32)
            model_audio /= 32768.0
        mlx_dir = model_dir.parent / (model_dir.name + "-mlx")
        config, weights = MLX_MODELS[model_dir.name]
        verify_model_file(mlx_dir, "config.json", *config)
        verify_model_file(mlx_dir, *weights)
        for other in ("model.safetensors", "weights.safetensors", "weights.npz"):
            if other != weights[0] and (mlx_dir / other).exists():
                raise ValueError(f"请移除当前 MLX 模型目录里的其他权重文件：{other}")
        transcription = mlx_whisper.transcribe(
            model_audio, path_or_hf_repo=str(mlx_dir), language=language, verbose=None,
            initial_prompt=" ".join(value for value in (prompt, hotwords) if value) or None,
            word_timestamps=True,
        )
        info = SimpleNamespace(language=transcription["language"])
        segments = (SimpleNamespace(start=row["start"], end=row["end"], text=row["text"],
                                    words=[SimpleNamespace(**word) for word in row.get("words", [])])
                    for row in transcription["segments"])
    else:
        from faster_whisper import WhisperModel
        model = WhisperModel(str(model_dir), device=device, device_index=device_index,
                             compute_type=compute_type,
                             local_files_only=True, cpu_threads=int(options.get("cpuThreads", 8)))
        segments, info = model.transcribe(model_audio, beam_size=3, vad_filter=options.get("vadFilter", True),
                                          hotwords=hotwords, language=language,
                                          initial_prompt=prompt, word_timestamps=True)
    emit("language", language=info.language)
    if info.language == "zh":
        from opencc import OpenCC
        convert = OpenCC("t2s").convert
    else:
        def convert(text):
            return text
    def display_text(text):
        return correct_timed_words(convert(text), [], correction_pattern, replacements)[0]

    result = None if options.get("_streamSegmentsOnly") else []
    last_progress = -1
    emitted = False
    try:
        for segment in segments:
            if not isfinite(segment.start) or not isfinite(segment.end):
                continue
            if boundary and segment.start + offset < boundary:
                row = trim_overlap(segment, offset, boundary, options.get("_resumeText", ""), display_text)
                if row is None:
                    continue
                start, end, text, words = row
            else:
                start, end, text = segment.start + offset, segment.end + offset, segment.text.strip()
                words = segment.words or []
            start, end = max(0.0, start), min(duration, end)
            if start >= end:
                continue
            if text:
                text = convert(text)
                aligned = timed_words(words, offset, start, end, text, convert)
                text, aligned = correct_timed_words(text, aligned, correction_pattern, replacements)
                row = dict(start=start, end=end, text=text, words=aligned)
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
    if sys.platform == "darwin" and device == "metal":
        return dict(recognize(output, folder, "metal", duration, options, "mlx"), **metadata)
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
