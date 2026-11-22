"""Convert the first audio stream to a seekable WAV and emit JSON lines."""
import json
import os
from pathlib import Path
import site
import subprocess
import sys
import wave


def emit(kind, **fields):
    print(json.dumps({"kind": kind, **fields}, ensure_ascii=True), flush=True)


def recognize(audio, model_dir, device, duration, options):
    from faster_whisper import WhisperModel
    emit("status", message=f"加载模型 · {device.upper()}")
    model = WhisperModel(str(model_dir), device=device,
                         compute_type="int8_float16" if device == "cuda" else "int8",
                         local_files_only=True, cpu_threads=int(options.get("cpuThreads", 8)))
    segments, info = model.transcribe(str(audio), beam_size=3, vad_filter=options.get("vadFilter", True),
                                      language=options.get("language") if options.get("language") in {"zh","en","ja"} else None)
    result = []
    for segment in segments:
        text = segment.text.strip()
        if text:
            result.append(dict(start=segment.start, end=segment.end, text=text))
        emit("status", message=f"正在转写 · {device.upper()} · {min(99, int(100 * segment.end / duration))}%")
    return dict(segments=result, duration=duration, language=info.language, device=device)


def main():
    options = json.loads(sys.argv[4]) if len(sys.argv)>4 else {}
    source, folder, output = map(Path, sys.argv[1:4])
    for root in site.getsitepackages():
        for folder_bin in Path(root).glob("nvidia/*/bin"):
            os.environ["PATH"] = str(folder_bin) + os.pathsep + os.environ["PATH"]
            handles.append(os.add_dll_directory(str(folder_bin)))
    if not (folder / "model.bin").is_file():
        raise ValueError(f"找不到模型文件：{folder}")
    emit("status", message="正在提取音轨")
    process = subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-i", str(source), "-map",
                              "0:a:0", "-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le",
                              "-y", str(output)], capture_output=True)
    if process.returncode:
        error = process.stderr.decode("utf-8", errors="replace")
        raise ValueError("文件没有音轨" if "matches no streams" in error else "音频解码失败：" + error[-400:])
    with wave.open(str(output), "rb") as wav:
        duration = wav.getnframes() / wav.getframerate()
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
    import ctranslate2
    device = options.get("computeDevice","auto")
    if device=="cuda" or (device=="auto" and ctranslate2.get_cuda_device_count()):
        try:
            return dict(recognize(output, folder, "cuda", duration, options), **metadata)
        except RuntimeError as err:
            if device=="cuda":
                raise
            emit("status", message="GPU 无法完成转写，正在使用 CPU", detail=str(err))
    return dict(recognize(output, folder, "cpu", duration, options), **metadata)


if __name__ == "__main__":
    handles = []
    try:
        emit("result", **main())
    except Exception as exc:
        emit("error", message=str(exc))
        sys.exit(1)
