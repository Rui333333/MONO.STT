# Windows 软件更新

MONO.STT 在启动后检查更新，也可以在「偏好设置 → 通用 → 检查更新」手动检查。新版确认后，应用会下载并验证签名，由 Windows 安装程序完成升级。

Windows 安装包把原生启动器、WebView2 引导组件、Tauri 界面和应用资源封在一个 EXE 中。手动安装可选择目录，覆盖安装沿用注册表里记录的目录。`setup.exe /s` 使用静默模式，可用退出码判断结果。

## 本机生成发布包

运行 `pwsh -File .\build-windows.ps1 -Slim -Signed`。脚本使用 `TAURI_SIGNING_PRIVATE_KEY` 和 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。若未配置环境变量，Windows 下会读取 `%LOCALAPPDATA%\MONO.STT\Signing\updater.key` 和同目录下由当前 Windows 用户加密的 `updater-password.dpapi`。

从 `src-tauri/target/release/bundle/windows` 取出本次构建的 `*-setup.exe`、对应的 `.sig` 和 `latest.json`，一起上传至和版本号一致的 GitHub Release。例如版本 `0.1.4` 对应标签 `v0.1.4`。可以在发布前把 `latest.json` 的 `notes` 改成这一版的更新说明。

## Windows 发布者签名

`-Signed` 生成 Tauri 更新签名。要让 Windows 验证发布者，需再接入受信任的 Authenticode 签名身份：

```powershell
pwsh -File .\build-windows.ps1 -Slim -Signed -WindowsSignScript C:\Signing\sign.ps1
```

签名脚本接收待签 EXE 的绝对路径作为首个参数，调用证书提供方的工具添加 Authenticode 签名和 RFC 3161 时间戳。构建会验证各文件签名，先签主程序、安装界面和维护程序，再封装并签署最终安装包，最后生成 Tauri 更新签名和 `latest.json`。

发布前检查安装包和已安装的主程序、维护程序签名，并实测安装、卸载和自动更新。`Get-AuthenticodeSignature` 应返回 `Valid`。重新签署安装包会改变文件字节，需要重新生成 Tauri 更新签名。新签名的应用仍可能在信誉积累前出现 SmartScreen 提示。

## 使用 GitHub Actions 打包 Windows 和 macOS

### 试打包

在 GitHub 的 **Actions → Windows and macOS packages → Run workflow** 选择要打包的分支。工作流从同一提交构建 Windows x64 和 macOS Apple Silicon 安装包。成功后在该运行记录的 **Artifacts** 下载 `MONO.STT-Windows-x64` 和 `MONO.STT-macOS-arm64`。试打包无需更新签名密钥，也不会创建 Release。

首次使用前，先把统一工作流合入仓库默认分支。 [GitHub 手动运行工作流说明](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow)

### 正式发布

在仓库的 Actions Secrets 配置 `TAURI_SIGNING_PRIVATE_KEY`（私钥文件内容）和 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`（私钥密码）。私钥和密码请单独备份。应用已内置对应公钥，后续发布必须沿用同一套密钥。

同步更新 `src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml` 和 `src-tauri/Cargo.lock` 内的应用版本。将版本提交推送到 GitHub，再给该提交打对应标签。例如 `0.1.6` 对应 `v0.1.6`。

两端都构建成功后，工作流将 EXE、DMG、Windows 更新签名 `.sig` 和 `latest.json` 放到同一个 GitHub Release 草稿。核对后手动发布为 Latest。Windows 在线更新继续使用 `latest.json`，macOS 通过 DMG 安装，当前仍需手动升级。

## 验证升级

现有 `0.1.3` 用户手动覆盖安装首个带更新功能的版本。之后用两个递增版本实测，确认应用可以检查、下载、安装并重新打开，已有转录记录、设置和模型也能正常读取。保留相同的应用标识、安装模式和安装目录。
