param([switch]$PrepareOnly, [switch]$BundleOnly, [switch]$Slim, [switch]$Signed)
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot

$target = Join-Path $PSScriptRoot 'src-tauri/target'
$staging = Join-Path $target 'windows-bundle'
$runtime = Join-Path $staging 'runtime'
if ($Slim) {
    $runtime = Join-Path $staging 'runtime-slim'
}
$pythonArchive = Join-Path $staging 'python-3.12.10-embed-amd64.zip'
$expectedHash = '4ACBED6DD1C744B0376E3B1CF57CE906F9DC9E95E68824584C8099A63025A3C3'

if (-not $BundleOnly) {
    if (!(Test-Path -LiteralPath $runtime)) {
        New-Item -ItemType Directory -Path $staging -Force | Out-Null
        if (!(Test-Path -LiteralPath $pythonArchive)) {
            $download = "$pythonArchive.part"
            curl.exe --fail --silent --show-error --location --retry 2 --max-time 120 `
                'https://www.python.org/ftp/python/3.12.10/python-3.12.10-embed-amd64.zip' -o $download
            if ($LASTEXITCODE -ne 0) { throw 'Python 下载失败。' }
            if ((Get-FileHash -LiteralPath $download).Hash -ne $expectedHash) {
                throw 'Python 嵌入包的 SHA-256 校验失败。'
            }
            Move-Item -LiteralPath $download -Destination $pythonArchive
        }
        if ((Get-FileHash -LiteralPath $pythonArchive).Hash -ne $expectedHash) {
            throw 'Python 嵌入包的 SHA-256 校验失败。'
        }

        $buildRuntime = Join-Path $staging ("runtime-" + [guid]::NewGuid())
        Expand-Archive -LiteralPath $pythonArchive -DestinationPath $buildRuntime
        $sitePackages = Join-Path $buildRuntime 'Lib/site-packages'
        $requirements = Join-Path $staging 'windows-requirements.txt'
        # CTranslate2's CUDA convolution does not require the optional cuDNN wheel.
        Get-Content -LiteralPath 'backend/requirements.txt' |
            Where-Object { $_ -notmatch '^nvidia-cudnn-cu12==' -and
                (!$Slim -or $_ -notmatch '^nvidia-') } |
            Set-Content -LiteralPath $requirements -Encoding ascii
        python -m pip --disable-pip-version-check install --only-binary=:all: --no-compile `
            --platform win_amd64 --python-version 3.12 --index-url https://pypi.org/simple `
            --target $sitePackages -r $requirements
        if ($LASTEXITCODE -ne 0) { throw '转写运行库安装失败。' }
        @('python312.zip', '.', 'Lib/site-packages', 'import site') |
            Set-Content -LiteralPath (Join-Path $buildRuntime 'python312._pth') -Encoding ascii

        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
        $visualStudio = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        $redist = Get-ChildItem -Path (Join-Path $visualStudio 'VC/Redist/MSVC/*/x64/Microsoft.VC*.CRT') -Directory |
            Sort-Object FullName -Descending | Select-Object -First 1
        if (!$redist) { throw '找不到 Visual Studio 的 x64 C++ 可再发行运行库。' }
        Copy-Item -Path (Join-Path $redist.FullName '*.dll') -Destination $buildRuntime
        $embedded = Join-Path $buildRuntime 'python.exe'
        & $embedded -I -B -c 'import av, ctranslate2, faster_whisper, numpy, onnxruntime'
        if ($LASTEXITCODE -ne 0) { throw '嵌入式推理环境验证失败。' }
        Move-Item -LiteralPath $buildRuntime -Destination $runtime
    }

}
if ($PrepareOnly) { return }
if (!(Test-Path -LiteralPath (Join-Path $runtime 'python.exe'))) { throw '请先准备 Windows 推理运行库。' }

$config = Get-Content -LiteralPath 'src-tauri/tauri.conf.json' -Raw | ConvertFrom-Json
$previousSigned = $env:MONO_SIGNED_UPDATE_BUILD
$previousKey = $env:TAURI_SIGNING_PRIVATE_KEY
$previousKeyPath = $env:TAURI_SIGNING_PRIVATE_KEY_PATH
$previousPassword = $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD
try {
    if ($Signed) {
        if (!$env:TAURI_SIGNING_PRIVATE_KEY -and !$env:TAURI_SIGNING_PRIVATE_KEY_PATH) {
            $keyDir = Join-Path $env:LOCALAPPDATA 'MONO.STT/Signing'
            $key = Join-Path $keyDir 'updater.key'
            $password = Join-Path $keyDir 'updater-password.dpapi'
            if (!(Test-Path -LiteralPath $key) -or !(Test-Path -LiteralPath $password)) {
                throw '缺少更新签名私钥。请配置 TAURI_SIGNING_PRIVATE_KEY 和 TAURI_SIGNING_PRIVATE_KEY_PASSWORD。'
            }
            if ((Get-Content -LiteralPath "$key.pub" -Raw).Trim() -ne $config.plugins.updater.pubkey) {
                throw '本机签名密钥与应用内公钥不匹配。'
            }
            $env:TAURI_SIGNING_PRIVATE_KEY_PATH = $key
            $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ConvertFrom-SecureString -AsPlainText (
                (Get-Content -LiteralPath $password -Raw).Trim() | ConvertTo-SecureString
            )
        }
        $env:MONO_SIGNED_UPDATE_BUILD = '1'
    } else {
        $env:MONO_SIGNED_UPDATE_BUILD = $null
    }

    if (!$BundleOnly) {
        cargo build --release --locked --manifest-path src-tauri/Cargo.toml
        if ($LASTEXITCODE -ne 0) { throw '应用构建失败。' }
    }
    cargo build --release --locked --manifest-path installer/Cargo.toml --target-dir $target
    if ($LASTEXITCODE -ne 0) { throw '安装引导器构建失败。' }

    $webview = Join-Path $staging 'MicrosoftEdgeWebView2Setup.exe'
    if (!(Test-Path -LiteralPath $webview)) {
        $cache = Join-Path $env:LOCALAPPDATA 'tauri/MicrosoftEdgeWebview2Setup.exe'
        if (Test-Path -LiteralPath $cache) {
            Copy-Item -LiteralPath $cache -Destination $webview
        } else {
            New-Item -ItemType Directory -Path $staging -Force | Out-Null
            Invoke-WebRequest -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile "$webview.part"
            Move-Item -LiteralPath "$webview.part" -Destination $webview
        }
    }
    $authenticode = Get-AuthenticodeSignature -LiteralPath $webview
    if ($authenticode.Status -ne 'Valid' -or $authenticode.SignerCertificate.Subject -notmatch 'Microsoft Corporation') {
        throw 'WebView2 安装程序缺少有效的 Microsoft 签名。'
    }

    $version = $config.version
    $bundle = Join-Path $target 'release/bundle/windows'
    New-Item -ItemType Directory -Path $bundle -Force | Out-Null
    $name = "$($config.productName)_${version}_x64-setup.exe"
    $result = Join-Path $bundle $name
    $build = Join-Path $target 'release'
    python installer/pack.py $build $runtime $webview $version $result
    if ($LASTEXITCODE -ne 0) { throw '无法封装 Windows 安装包。' }

    if ($Signed) {
        $env:npm_config_registry = 'https://registry.npmjs.org'
        npm exec --yes --package=@tauri-apps/cli@2.12.6 -- tauri signer sign --app-version $version $result
        if ($LASTEXITCODE -ne 0) { throw '安装包签名失败。' }
        $signature = (Get-Content -LiteralPath "$result.sig" -Raw).Trim()
        if (!$signature) { throw '签名为空。' }
        $release = 'https://github.com/Rui333333/MONO.STT/releases/download'
        @{
            version = $version
            notes = "MONO.STT $version"
            pub_date = [DateTimeOffset]::UtcNow.ToString('o')
            platforms = @{
                'windows-x86_64' = @{
                    url = "$release/v$version/$([Uri]::EscapeDataString($name))"
                    signature = $signature
                }
            }
        } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $bundle 'latest.json') -Encoding utf8NoBOM
    }
    Get-Item -LiteralPath $result | Select-Object FullName, Length
} finally {
    $env:MONO_SIGNED_UPDATE_BUILD = $previousSigned
    $env:TAURI_SIGNING_PRIVATE_KEY = $previousKey
    $env:TAURI_SIGNING_PRIVATE_KEY_PATH = $previousKeyPath
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $previousPassword
}
