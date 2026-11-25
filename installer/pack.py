"""Pack the Windows launcher, installer UI, WebView bootstrap and app payload."""
from pathlib import Path
import shutil
import struct
import sys
import tempfile
import zipfile


def pack(build: Path, runtime: Path, webview: Path, version: str, output: Path) -> None:
    with tempfile.TemporaryDirectory(dir=output.parent) as staging:
        staging = Path(staging)
        payload = staging / "payload.zip"
        with zipfile.ZipFile(payload, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
            archive.writestr("version.txt", version)
            archive.write(build / "mono-stt.exe", "mono-stt.exe")
            for path in sorted(runtime.rglob("*")):
                if path.is_file():
                    archive.write(path, (Path("runtime") / path.relative_to(runtime)).as_posix())
        parts = [build / "mono-stt-setup.exe", build / "mono-stt-installer.exe", webview, payload]
        lengths = [part.stat().st_size for part in parts]
        setup = staging / output.name
        with setup.open("wb") as dest:
            for part in parts:
                with part.open("rb") as source:
                    shutil.copyfileobj(source, dest, 1024 * 1024)
            dest.write(struct.pack("<8sQQQQ", b"MONOSTT1", *lengths))
        setup.replace(output)


if __name__ == "__main__":
    build_dir, runtime_dir, webview_path, version, output_path = sys.argv[1:]
    pack(Path(build_dir), Path(runtime_dir), Path(webview_path), version, Path(output_path))
