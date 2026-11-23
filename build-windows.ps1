param([switch]$PrepareOnly, [switch]$BundleOnly, [switch]$Slim)
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
npm exec --yes --package=@tauri-apps/cli@2.12.6 -- tauri build --config $packageConfig --bundles nsis -- --offline --locked
if ($LASTEXITCODE -ne 0) { throw 'Windows 安装程序构建失败。' }
