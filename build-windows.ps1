param([switch]$PrepareOnly, [switch]$BundleOnly, [switch]$Slim, [switch]$Signed)
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot

$target = Join-Path $PSScriptRoot 'src-tauri/target'
$staging = Join-Path $target 'windows-bundle'
$runtime = Join-Path $staging 'runtime'
$packageConfig = Join-Path $staging 'bundle.json'
if ($Slim) {
    $runtime = Join-Path $staging 'runtime-slim'
    $packageConfig = Join-Path $staging 'bundle-slim.json'
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

    $source = (Resolve-Path -LiteralPath $runtime).Path.Replace('\', '/')
    @{
        bundle = @{
            active = $true
            targets = @('nsis')
            resources = @{ $source = 'runtime/' }
            windows = @{
                webviewInstallMode = @{ type = 'embedBootstrapper' }
                nsis = @{
                    installMode = 'currentUser'
                    languages = @('SimpChinese', 'English')
                    compression = $(if ($Slim) { 'lzma' } else { 'zlib' })
                }
            }
        }
    } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $packageConfig -Encoding utf8
}
if ($PrepareOnly) { return }

$env:npm_config_registry = 'https://registry.npmjs.org'
$tauriArgs = @('tauri', 'build', '--config', $packageConfig, '--bundles', 'nsis')
$previousSigned = $env:MONO_SIGNED_UPDATE_BUILD
$previousKey = $env:TAURI_SIGNING_PRIVATE_KEY
$previousPassword = $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD
$config = Get-Content -LiteralPath 'src-tauri/tauri.conf.json' -Raw | ConvertFrom-Json
try {
    if ($Signed) {
        if (!$env:TAURI_SIGNING_PRIVATE_KEY) {
            $keyDir = Join-Path $env:LOCALAPPDATA 'MONO.STT/Signing'
            $key = Join-Path $keyDir 'updater.key'
            $password = Join-Path $keyDir 'updater-password.dpapi'
            if (!(Test-Path -LiteralPath $key) -or !(Test-Path -LiteralPath $password)) {
                throw '缺少更新签名私钥。请配置 TAURI_SIGNING_PRIVATE_KEY 和 TAURI_SIGNING_PRIVATE_KEY_PASSWORD。'
            }
            if ((Get-Content -LiteralPath "$key.pub" -Raw).Trim() -ne $config.plugins.updater.pubkey) {
                throw '本机签名密钥与应用内公钥不匹配。'
            }
            $env:TAURI_SIGNING_PRIVATE_KEY = $key
            $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ConvertFrom-SecureString -AsPlainText (
                (Get-Content -LiteralPath $password -Raw).Trim() | ConvertTo-SecureString
            )
        }
        $env:MONO_SIGNED_UPDATE_BUILD = '1'
        $signingConfig = Join-Path $staging 'bundle-signed.json'
        '{"bundle":{"createUpdaterArtifacts":true}}' |
            Set-Content -LiteralPath $signingConfig -Encoding utf8NoBOM
        $tauriArgs += @('--config', $signingConfig)
    } else {
        $env:MONO_SIGNED_UPDATE_BUILD = $null
    }
    npm exec --yes --package=@tauri-apps/cli@2.12.6 -- @tauriArgs -- --offline --locked
    if ($LASTEXITCODE -ne 0) { throw 'Windows 安装程序构建失败。' }
} finally {
    $env:MONO_SIGNED_UPDATE_BUILD = $previousSigned
    $env:TAURI_SIGNING_PRIVATE_KEY = $previousKey
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $previousPassword
}

if ($Signed) {
    $version = $config.version
    $nsis = Join-Path $target 'release/bundle/nsis'
    $name = "$($config.productName)_${version}_x64-setup.exe"
    $installer = Join-Path $nsis $name
    $signature = (Get-Content -LiteralPath "$installer.sig" -Raw).Trim()
    if (!(Test-Path -LiteralPath $installer) -or !$signature) { throw '找不到已签名的安装程序。' }
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
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $nsis 'latest.json') -Encoding utf8NoBOM
}
