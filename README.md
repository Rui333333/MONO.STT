# MONO.STT

MONO.STT 是一款 Windows 桌面转录工具。导入本地音频、视频，或直接使用麦克风录音，应用会在本机生成带时间戳的逐字稿。逐字稿支持回听、校对和导出，适合整理访谈、课程和会议记录。

## 功能

- **本地转录** — 支持 Whisper large-v3-turbo 和 Whisper large-v3，可使用 CPU 或兼容的 NVIDIA CUDA 环境。
- **首次设置** — 检测设备、下载或导入已有模型，并用内置普通话音频试转录。
- **时间轴校对** — 点击时间戳跳转回听，支持播放速度、当前段落跟随和回到当前朗读行。
- **导出** — 单条记录可导出 TXT、Markdown 和 SRT。多条记录可将 TXT 或 SRT 打包为 ZIP，也可按需发布到飞书云文档。
- **本地归档** — 转录、设置和关联信息保存在软件目录下的 `data/mono.db`。回听音轨和录音保存在 `data/audio/` 和 `data/recordings/`。

## 安装与使用

1. 从 [GitHub Releases](https://github.com/Rui333333/MONO.STT/releases) 获取 Windows x64 安装包并安装。
2. 首次打开时选择计算设备和语音模型。可以下载模型，也可以选择已有的模型目录。示例试转录通过后即可进入工作台。
3. 把音视频拖入窗口，确认后开始转录。完成后可在时间轴校对文字并导出。

语音识别和回听在本机运行。下载模型、检查更新或主动发布到飞书时需要网络。删除转录记录会清理应用保存的配套音频，导入原件保留在原位置。

## 从源码构建

在 Windows x64 上准备 PowerShell 7、Python、Rust MSVC 工具链、Visual Studio C++ 生成工具和 Windows SDK。从仓库根目录运行：

```powershell
pwsh -File .\build-windows.ps1 -Slim
```

安装包输出到 `src-tauri/target/release/bundle/windows/`。精简包包含本地推理运行环境。使用 GPU 加速时需要可用的 NVIDIA CUDA 运行库。

| 路径 | 内容 |
| --- | --- |
| `ui/` | 桌面界面、首次设置和转录校对 |
| `src-tauri/` | Tauri 桌面应用、SQLite 存储和系统集成 |
| `backend/` | Python 转录引擎 |
| `installer/` | Windows 安装器和打包工具 |

发布、Windows 代码签名和自动更新配置见 [UPDATES.md](UPDATES.md)。

## 许可证

MONO.STT 项目代码使用 [MIT License](LICENSE)。第三方依赖、字体和模型权重适用各自的许可证。
