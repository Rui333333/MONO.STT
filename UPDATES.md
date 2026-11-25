# Windows 软件更新

MONO.STT 在启动后检查更新，也可以在「偏好设置 → 通用 → 检查更新」手动检查。新版确认后，应用会下载并验证签名，由 Windows 安装程序完成升级。

Windows 安装包把原生启动器、WebView2 引导组件、Tauri 界面和应用资源封在一个 EXE 中。手动安装可选择目录，覆盖安装沿用注册表里记录的目录。`setup.exe /s` 使用静默模式，可用退出码判断结果。

## 本机生成发布包

运行 `pwsh -File .\build-windows.ps1 -Slim -Signed`。脚本使用 `TAURI_SIGNING_PRIVATE_KEY` 和 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。若未配置环境变量，Windows 下会读取 `%LOCALAPPDATA%\MONO.STT\Signing\updater.key` 和同目录下由当前 Windows 用户加密的 `updater-password.dpapi`。

从 `src-tauri/target/release/bundle/windows` 取出本次构建的 `*-setup.exe`、对应的 `.sig` 和 `latest.json`，一起上传至和版本号一致的 GitHub Release。例如版本 `0.1.4` 对应标签 `v0.1.4`。可以在发布前把 `latest.json` 的 `notes` 改成这一版的更新说明。

## 使用 GitHub Actions

在仓库的 Actions Secrets 配置 `TAURI_SIGNING_PRIVATE_KEY`（私钥文件内容）和 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`（私钥密码）。私钥和密码请单独备份。应用已内置对应公钥，后续发布必须沿用同一套密钥。

发布时同步修改 `src-tauri/tauri.conf.json` 与 `src-tauri/Cargo.toml` 的版本，更新 `src-tauri/Cargo.lock`，然后推送同版本标签。工作流会构建签名包并创建草稿 Release。核对附件后，将该 Release 发布为 Latest，已安装带更新功能的版本才会收到提示。

## 验证升级

现有 `0.1.3` 用户手动覆盖安装首个带更新功能的版本。之后用两个递增版本实测，确认应用可以检查、下载、安装并重新打开，已有转录记录、设置和模型也能正常读取。保留相同的应用标识、安装模式和安装目录。
