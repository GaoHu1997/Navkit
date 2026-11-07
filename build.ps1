<#
.SYNOPSIS
    一键构建 NavKit 绿色版（免安装单文件 exe）。

.DESCRIPTION
    执行：npm install -> 前端构建 -> tauri build -> 把 exe 与资源整理成绿色版文件夹。
    产物目录为 .\dist-portable，包含 navkit.exe、data\（便携数据目录）与说明文件。
    整个文件夹拷走即用，不写 AppData、不写注册表。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\build.ps1
    powershell -ExecutionPolicy Bypass -File .\build.ps1 -SkipInstall
#>
[CmdletBinding()]
param(
    # 跳过 npm install（依赖已就绪时更快）
    [switch]$SkipInstall,
    # 构建后不启动程序
    [switch]$NoRun,
    # 附加的 Cargo 参数
    [string]$CargoArgs = ""
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

Write-Host "==========================================" -ForegroundColor Cyan
Write-Host " NavKit — 绿色版构建" -ForegroundColor Cyan
Write-Host "==========================================" -ForegroundColor Cyan

# ---------------------------------------------------------------- 依赖检查
function Test-Command($name) {
    return [bool](Get-Command $name -ErrorAction SilentlyContinue)
}

if (-not (Test-Command "node")) {
    throw "未找到 node，请先安装 Node.js 18+"
}
if (-not (Test-Command "npm")) {
    throw "未找到 npm，请先安装 Node.js 18+"
}
if (-not (Test-Command "cargo")) {
    throw "未找到 cargo，请先安装 Rust（https://rustup.rs/）"
}

$nodeVer = (node --version)
$cargoVer = (cargo --version)
Write-Host "Node.js : $nodeVer" -ForegroundColor DarkGray
Write-Host "Rust   : $cargoVer" -ForegroundColor DarkGray

# ---------------------------------------------------------------- 安装依赖
if (-not $SkipInstall) {
    Write-Host "`n[1/4] 安装前端依赖 ..." -ForegroundColor Yellow
    npm install
    if ($LASTEXITCODE -ne 0) { throw "npm install 失败" }
} else {
    Write-Host "`n[1/4] 跳过前端依赖安装" -ForegroundColor DarkGray
}

# ---------------------------------------------------------------- 前端构建
# 注意：tauri build 会自动执行 beforeBuildCommand（= npm run build），
# 若这里再跑一次会生成两套 hash 不同的产物，导致 dist\ 与 exe 内嵌不一致，
# 也让“是否最新”无法核对。故这里只做类型检查，前端产物统一交给 tauri 构建。
Write-Host "`n[2/4] 前端类型检查（产物由 tauri build 统一生成）..." -ForegroundColor Yellow
npx tsc --noEmit
if ($LASTEXITCODE -ne 0) { throw "前端类型检查失败" }

# ---------------------------------------------------------------- 后端构建
Write-Host "`n[3/4] 构建 Rust 可执行文件 + 前端资源 ..." -ForegroundColor Yellow
$npxArgs = @("tauri", "build")
if ($CargoArgs) { $npxArgs += @("--", $CargoArgs) }
& npx @npxArgs
if ($LASTEXITCODE -ne 0) { throw "Tauri 构建失败" }

# ---------------------------------------------------------------- 整理绿色版
Write-Host "`n[4/4] 整理绿色版目录 ..." -ForegroundColor Yellow

$srcExe = Join-Path $PSScriptRoot "src-tauri\target\release\navkit.exe"
if (-not (Test-Path $srcExe)) {
    throw "未找到构建产物：$srcExe"
}

$outDir = Join-Path $PSScriptRoot "dist-portable"

# 原地更新程序文件，绝不删除用户 data（包括密码密钥、收藏和备份）。
# 若正在运行的 exe 无法替换，Copy-Item 失败即可，用户数据仍保持原样。
New-Item -ItemType Directory -Path $outDir -Force | Out-Null
Copy-Item $srcExe (Join-Path $outDir "navkit.exe") -Force
foreach ($relative in @("data", "data\logs", "data\cache")) {
    New-Item -ItemType Directory -Path (Join-Path $outDir $relative) -Force | Out-Null
}
Write-Host "  原地更新，已有连接、密钥、收藏和备份保持不变" -ForegroundColor DarkGray

# WebView2 离线引导程序：无网络、无预装 WebView2 时也能双击运行。
$webview2 = Join-Path $outDir "MicrosoftEdgeWebview2Setup.exe"
$wvUrl = "https://go.microsoft.com/fwlink/p/?LinkId=2124703"
try {
    Write-Host "  下载 WebView2 离线引导程序 ..." -ForegroundColor DarkGray
    Invoke-WebRequest -Uri $wvUrl -OutFile $webview2 -UseBasicParsing -TimeoutSec 180
    Write-Host "  WebView2 引导程序已就绪" -ForegroundColor DarkGray
} catch {
    Write-Warning "无法下载 WebView2 引导程序（需要联网时手动运行官方安装包）：$wvUrl"
    if (Test-Path $webview2) { Remove-Item $webview2 -Force }
}

$readmePath = Join-Path $outDir "README-portable.txt"
$src = Get-ChildItem -LiteralPath (Join-Path $PSScriptRoot "assets") -Filter *.txt | Select-Object -First 1
Copy-Item -LiteralPath $src.FullName -Destination $readmePath -Force

# ---------------------------------------------------------------- 构建指纹
# 每次打包写入构建时间 / exe 指纹 / 源码清单指纹，便于核对“运行的是不是最新版”。
Write-Host "`n写入构建指纹 ..." -ForegroundColor DarkGray
$exeHash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $outDir "navkit.exe")).Hash
$srcFiles = Get-ChildItem -Path (Join-Path $PSScriptRoot "src"), (Join-Path $PSScriptRoot "src-tauri\src") -Recurse -File -ErrorAction SilentlyContinue
$srcFingerprint = ($srcFiles | Sort-Object FullName | ForEach-Object { "$($_.FullName)|$($_.LastWriteTimeUtc.Ticks)|$($_.Length)" }) -join "`n"
$sha = [System.Security.Cryptography.SHA256]::Create()
$srcHash = -join (($sha.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($srcFingerprint))) | ForEach-Object { $_.ToString("x2") })

# 从 exe 二进制中提取真正内嵌的前端资源名（tauri 以明文记录 assets 路径）
$exeBytes = [System.IO.File]::ReadAllBytes((Join-Path $outDir "navkit.exe"))
$exeText = [System.Text.Encoding]::ASCII.GetString($exeBytes)
$embeddedAssets = (([regex]::Matches($exeText, '(?:index|window)-[A-Za-z0-9_-]{8}\.js') |
    ForEach-Object { $_.Value } | Sort-Object -Unique) -join ", ")
if (-not $embeddedAssets) { $embeddedAssets = "(未识别)" }
$info = @(
    "NavKit 绿色版构建信息"
    "构建时间   : $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')"
    "版本       : $((Get-Content -Raw (Join-Path $PSScriptRoot 'package.json') | ConvertFrom-Json).version)"
    "exe SHA256 : $exeHash"
    "源码指纹   : $srcHash"
    "前端指纹   : $embeddedAssets"
)
$infoPath = Join-Path $outDir "data\build-info.txt"
Set-Content -LiteralPath $infoPath -Value $info -Encoding UTF8
Write-Host "  $infoPath" -ForegroundColor DarkGray
$info | ForEach-Object { Write-Host "  $_" -ForegroundColor DarkGray }

# ---------------------------------------------------------------- 产物一致性校验
# tauri build 会再跑一次 beforeBuildCommand（vite），其产物才是真正嵌入 exe 的。
# 以 exe 内嵌的 assets 文件名与当前 dist 对比，不一致说明 dist 已过期，必须重新打包。
$distAssets = @(Get-ChildItem (Join-Path $PSScriptRoot "dist\assets") -Filter *.js | Sort-Object Name | ForEach-Object { $_.Name })
$distSet = ($distAssets | Sort-Object) -join ","
$embSet = (($embeddedAssets -split ", ") | Sort-Object) -join ","
if ($embSet -ne $distSet) {
    Write-Warning "前端产物与 exe 不一致！"
    Write-Warning "  exe 内嵌 : $embSet"
    Write-Warning "  dist 目录: $distSet"
    Write-Warning "  请重新执行打包（dist 已被 tauri 的第二次 vite 覆盖，属正常现象，重跑即可对齐）"
} else {
    Write-Host "`n产物一致性校验通过：exe 内嵌 = dist 目录" -ForegroundColor Green
}

# ---------------------------------------------------------------- 完成
$sizeMb = [math]::Round((Get-Item $srcExe).Length / 1MB, 2)
Write-Host "`n==========================================" -ForegroundColor Green
Write-Host " 构建完成！" -ForegroundColor Green
Write-Host " 绿色版目录 : $outDir" -ForegroundColor Green
Write-Host " 可执行文件 : navkit.exe ($sizeMb MB)" -ForegroundColor Green
Write-Host "==========================================" -ForegroundColor Green

if (-not $NoRun) {
    Write-Host "`n正在启动 navkit.exe ..." -ForegroundColor Cyan
    Start-Process -FilePath (Join-Path $outDir "navkit.exe") -WorkingDirectory $outDir
}
