$ErrorActionPreference = "Stop"

$RepoUrl = "https://github.com/KaanAlper/anime-cli.git"
$ReleaseBinUrl = "https://github.com/KaanAlper/anime-cli/releases/latest/download/anime.exe"
$InstallDir = Join-Path $env:LOCALAPPDATA "anime-cli"
$BinPath = Join-Path $InstallDir "anime.exe"

Write-Host "✦ anime-cli Windows kurulumu başlatılıyor..." -ForegroundColor Magenta

function Ensure-WingetPkg($Id, $CmdName) {
    if (Get-Command $CmdName -ErrorAction SilentlyContinue) {
        return
    }
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        Write-Host "→ $Id kuruluyor..." -ForegroundColor Cyan
        winget install --id $Id -e --accept-source-agreements --accept-package-agreements
    } else {
        Write-Warning "'$CmdName' bulunamadı ve winget yok; lütfen manuel kurun ($Id)."
    }
}

Ensure-WingetPkg "charmbracelet.gum" "gum"
Ensure-WingetPkg "yt-dlp.yt-dlp" "yt-dlp"
Ensure-WingetPkg "Gyan.FFmpeg" "ffmpeg"
Ensure-WingetPkg "shinchiro.mpv" "mpv"

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

$Downloaded = $false
try {
    Write-Host "→ Hazır Windows ikilisi deneniyor..." -ForegroundColor Cyan
    Invoke-WebRequest -Uri $ReleaseBinUrl -OutFile $BinPath -UseBasicParsing
    if ((Get-Item $BinPath).Length -gt 100000) {
        $Downloaded = $true
        Write-Host "✔ İndirildi: $BinPath" -ForegroundColor Green
    }
} catch {
    Remove-Item -Force -ErrorAction SilentlyContinue $BinPath
}

if (-not $Downloaded) {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Ensure-WingetPkg "Rustlang.Rustup" "rustup"
        $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
    }
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        Ensure-WingetPkg "Git.Git" "git"
        $env:PATH = "$env:ProgramFiles\Git\cmd;$env:PATH"
    }
    $TmpDir = Join-Path $env:TEMP ("anime-cli-build-" + [guid]::NewGuid().ToString("N"))
    try {
        Write-Host "→ Kaynaktan derleniyor..." -ForegroundColor Cyan
        git clone --depth 1 $RepoUrl $TmpDir
        Push-Location $TmpDir
        cargo build --release
        Copy-Item -Force (Join-Path $TmpDir "target\release\anime.exe") $BinPath
        Pop-Location
        Write-Host "✔ Derlendi ve kuruldu: $BinPath" -ForegroundColor Green
    } finally {
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $TmpDir
    }
}

$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not $UserPath) { $UserPath = "" }
if ($UserPath.Split(';') -notcontains $InstallDir) {
    [Environment]::SetEnvironmentVariable("Path", ($UserPath.TrimEnd(';') + ";" + $InstallDir).TrimStart(';'), "User")
    $env:PATH = "$InstallDir;$env:PATH"
    Write-Host "→ PATH'e eklendi: $InstallDir" -ForegroundColor Cyan
}

Write-Host ""
Write-Host "✦ Kurulum tamamlandı! Yeni bir terminal açıp 'anime' yazabilirsiniz." -ForegroundColor Magenta
