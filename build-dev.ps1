# build-dev.ps1 — 编译 + 自动代码签名（自签证书 CN=warpdotsys reader-dev）
# 用法: powershell -ExecutionPolicy Bypass -File build-dev.ps1 [-Args "check|test|run|build --release"]
param([string]$Args)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path

# 1. 编译
Push-Location $root
if ($Args) {
    cargo $Args.Split(" ") | Out-Host
    $exit = $LASTEXITCODE
    if ($exit -ne 0) { Pop-Location; exit $exit }
} else {
    cargo build | Out-Host
    if ($LASTEXITCODE -ne 0) { Pop-Location; exit $LASTEXITCODE }
}
Pop-Location

# 2. 签名（debug/release 产物，若存在）
$cert = Get-ChildItem Cert:\CurrentUser\My | Where-Object {
    $_.Subject -like "*warpdotsys*" -and $_.NotAfter -gt (Get-Date)
} | Select-Object -First 1

if (-not $cert) {
    Write-Warning "未找到签名证书（CN=warpdotsys reader-dev），跳过签名"
    exit 0
}

$targets = @(
    "$root\target\debug\reader-dev.exe",
    "$root\target\release\reader-dev.exe"
)
foreach ($target in $targets) {
    if (Test-Path $target) {
        try {
            $sig = Set-AuthenticodeSignature -FilePath $target -Certificate $cert -ErrorAction Stop
            Write-Host "✅ 已签名: $target (状态: $($sig.Status))"
        } catch {
            # 文件可能被占用（如服务运行中）——尝试后报错但不中断
            Write-Warning "签名失败（可能被占用）: $target — $_"
        }
    }
}
Write-Host "✔ 构建+签名完成"
