# MONO.STT

MONO.STT 是一款 Windows 和 macOS 桌面转录工具。导入音频或视频，即可在本机生成带时间戳的逐字稿。转录结果可以回听、校对和导出，适合整理访谈、课程和会议记录。

## 下载与安装

| 平台 | 计算设备 | 0.1.7 安装包 |
| --- | --- | --- |
| Windows x64 | CPU、兼容的 NVIDIA CUDA | [下载 EXE](https://github.com/Rui333333/MONO.STT/releases/download/v0.1.7/MONO.STT_0.1.7_x64-setup.exe) |
| macOS 14 及以上，Apple Silicon（M 系列） | CPU | [下载 DMG](https://github.com/Rui333333/MONO.STT/releases/download/v0.1.7/MONO.STT_0.1.7_arm64.dmg) |

- **Windows** 双击 EXE 并按提示安装。可以在「偏好设置 → 通用 → 检查更新」手动检查新版。
- **macOS** 打开 DMG，将 MONO.STT 拖入「应用程序」。当前安装包尚未完成 Apple 公证，首次打开可能会出现系统提示。macOS 在线更新暂未启用。 [Apple 官方说明](https://support.apple.com/zh-cn/102445)

两个安装包都内置转录引擎。首次启动时按引导下载模型或选择已有模型目录，然后用内置普通话音频试转录。其他版本见 [GitHub Releases](https://github.com/Rui333333/MONO.STT/releases)。

## 功能

- **本地转录** 支持 Whisper Large v3 Turbo 和 Whisper Large v3。可拖入音视频，也可通过文件选择器导入。
- **时间轴校对** 点击时间戳跳转回听，可调整播放速度、开启滚动跟随并修改逐字稿。
- **文件导出** 单条记录可导出 TXT、Markdown 和 SRT。批量导出支持搜索、分页和跨页勾选，可将 TXT、SRT 或两种格式打包进一个 ZIP。
- **飞书云文档** 配置自建应用凭据后，可把转录内容写入飞书，更新或删除已关联的文档。

语音识别和回听在本机运行。下载模型、检查更新或主动发布到飞书时需要网络。删除转录记录会清理应用保存的配套音频，导入原件保留在原位置。

## 数据位置

| 平台 | 转录和设置 | 默认模型目录 |
| --- | --- | --- |
| Windows | 安装目录下的 `data/` | 安装目录下的 `models/` |
| macOS | `~/Library/Application Support/MONO.STT/data/` | `~/Library/Application Support/MONO.STT/models/` |

两个平台都可以在偏好设置中修改模型目录。转录记录保存在 `data/mono.db`，回听音轨保存在 `data/audio/`。

## 从源码构建

### Windows

在 Windows x64 上准备 PowerShell 7、Python、Rust MSVC 工具链、Visual Studio C++ 生成工具和 Windows SDK。从仓库根目录运行：

```powershell
pwsh -File .\build-windows.ps1 -Slim
```

安装包输出到 `src-tauri/target/release/bundle/windows/`。精简包包含本地推理运行环境。使用 GPU 加速时需要可用的 NVIDIA CUDA 运行库。

Windows 发布、代码签名和自动更新配置见 [UPDATES.md](UPDATES.md)。

### macOS

在 GitHub Actions 运行 **Windows and macOS packages**，从同一份代码生成 Windows x64 EXE 和 macOS Apple Silicon DMG。macOS 构建会打包 arm64 转录引擎，并挂载 DMG 运行 CPU 示例转录。试打包和标签发布见 [发布说明](UPDATES.md)。

## 项目结构

| 路径 | 内容 |
| --- | --- |
| `ui/` | 桌面界面、首次设置和转录校对 |
| `src-tauri/` | Tauri 桌面应用、SQLite 存储和系统集成 |
| `backend/` | Python 转录引擎 |
| `installer/` | Windows 安装器和打包工具 |

## 许可证

MONO.STT 项目代码使用 [MIT License](LICENSE)。第三方依赖、字体和模型权重适用各自的许可证。
