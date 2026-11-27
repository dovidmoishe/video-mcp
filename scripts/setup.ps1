<#
.SYNOPSIS
One-command Windows setup. Checks prerequisites, downloads the pinned FFmpeg and
Whisper files into .local, builds the release server, checks that it starts, and
prints agent registration commands. Safe to rerun; finished steps are skipped.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File scripts\setup.ps1
#>
param(
    # Build output directory inside the repository. Use a new one while an agent
    # still runs the previous executable (Windows locks it).
    [string]$TargetDirectory = 'target',
    # Skip the 78 MB speech model; transcription then reports model_unconfigured.
    [switch]$NoSpeechModel,
    # Check prerequisites and download dependencies only.
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue' # Invoke-WebRequest is far slower with a progress bar.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$local = Join-Path $root '.local'
# Pinned downloads. Hashes come from the GitHub release and Hugging Face LFS metadata.
$ffmpegName = 'ffmpeg-8.0-full_build-shared'
$ffmpegUrl = "https://github.com/GyanD/codexffmpeg/releases/download/8.0/$ffmpegName.zip"
$ffmpegSha256 = 'd2bfb598ec5d8d69c86cb198decc829fefb2c0acaa7ddd27ab5348139cce3dea'
$modelUrl = 'https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin'
$modelSha256 = '921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f'
$ffmpegDir = Join-Path $local $ffmpegName
$modelPath = Join-Path $local 'ggml-tiny.en.bin'
$exe = Join-Path $root "$TargetDirectory\release\video-mcp.exe"

function Write-Step([string]$Text) { Write-Host "==> $Text" -ForegroundColor Cyan }

function Get-Sha256([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }

function Save-Verified([string]$Url, [string]$Sha256, [string]$Path) {
    $partial = "$Path.partial"
    Write-Step "Downloading $Url"
    Invoke-WebRequest -Uri $Url -OutFile $partial -UseBasicParsing
    $actual = Get-Sha256 $partial
    if ($actual -ne $Sha256) {
        Remove-Item -LiteralPath $partial -Force
        throw "Checksum mismatch for $Url (expected $Sha256, got $actual)"
    }
    Move-Item -LiteralPath $partial -Destination $Path -Force
}

# Starts the server like an agent would, with only the registered VIDEO_* settings,
# and returns the number of tools it lists.
function Test-VideoServer([string]$Executable, [System.Collections.IDictionary]$Environment) {
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $Executable
    $info.UseShellExecute = $false
    $info.RedirectStandardInput = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    foreach ($name in @($info.EnvironmentVariables.Keys)) {
        if ($name -like 'VIDEO_*') { $info.EnvironmentVariables.Remove($name) }
    }
    foreach ($name in $Environment.Keys) { $info.EnvironmentVariables[$name] = $Environment[$name] }
    $info.StandardOutputEncoding = New-Object System.Text.UTF8Encoding $false
    # .NET Framework creates the child's stdin writer with the console input encoding and
    # writes its preamble at once. On a UTF-8 console (code page 65001) that is a byte-order
    # mark, which the server rejects as invalid JSON, so start with a BOM-free encoding.
    $consoleEncoding = $null
    try {
        $consoleEncoding = [Console]::InputEncoding
        [Console]::InputEncoding = New-Object System.Text.UTF8Encoding $false
    } catch {
        $consoleEncoding = $null # No console attached: no preamble is written.
    }
    try {
        $process = [Diagnostics.Process]::Start($info)
    } finally {
        if ($consoleEncoding) { [Console]::InputEncoding = $consoleEncoding }
    }
    $stderr = $process.StandardError.ReadToEndAsync() # Drain so the server never blocks on logging.
    # Write BOM-free UTF-8 lines ending in LF, as MCP stdio expects.
    $writer = New-Object System.IO.StreamWriter($process.StandardInput.BaseStream, (New-Object System.Text.UTF8Encoding $false))
    $writer.NewLine = "`n"
    $writer.AutoFlush = $true
    try {
        $send = { param($Message) $writer.WriteLine(($Message | ConvertTo-Json -Compress -Depth 6)) }
        $receive = {
            param($Id)
            while ($true) {
                $line = $process.StandardOutput.ReadLineAsync()
                if (-not $line.Wait(60000)) { throw 'The server did not answer within 60 seconds.' }
                if ($null -eq $line.Result) {
                    $null = $process.WaitForExit(5000)
                    throw "The server exited (code $($process.ExitCode)). Its log: $($stderr.Result)"
                }
                $message = $line.Result | ConvertFrom-Json
                if ($message.id -eq $Id) { return $message }
            }
        }
        & $send @{ jsonrpc = '2.0'; id = 1; method = 'initialize'; params = @{ protocolVersion = '2024-11-05'; capabilities = @{}; clientInfo = @{ name = 'video-mcp-setup'; version = '1' } } }
        $null = & $receive 1
        & $send @{ jsonrpc = '2.0'; method = 'notifications/initialized' }
        & $send @{ jsonrpc = '2.0'; id = 2; method = 'tools/list' }
        return @((& $receive 2).result.tools).Count
    } finally {
        if (-not $process.HasExited) {
            $writer.Close() # End of input: the server shuts down.
            if (-not $process.WaitForExit(10000)) { $process.Kill() }
        }
    }
}

if ($env:OS -ne 'Windows_NT') { throw 'This setup script supports Windows only.' }

# 1. Prerequisites: report everything that is missing at once.
Write-Step 'Checking prerequisites'
$missing = @()
if (-not (Get-Command cargo -ErrorAction SilentlyContinue) -or -not (Get-Command rustc -ErrorAction SilentlyContinue)) {
    $missing += 'Rust: winget install Rustlang.Rustup'
} elseif (-not ((& rustc -vV) -match '^host: x86_64-pc-windows-msvc')) {
    $missing += 'Rust x64 MSVC toolchain: rustup default stable-x86_64-pc-windows-msvc'
}
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vcTools = $null
if (Test-Path -LiteralPath $vswhere) {
    $vcTools = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
}
if (-not $vcTools) {
    $missing += 'Visual Studio C++ build tools: winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"'
}
$libclang = $null
foreach ($candidate in @($env:LIBCLANG_PATH, 'C:\Program Files\LLVM\bin')) {
    if ($candidate -and (Test-Path -LiteralPath (Join-Path $candidate 'libclang.dll'))) { $libclang = $candidate; break }
}
if (-not $libclang) { $missing += 'LLVM (libclang): winget install LLVM.LLVM' }
$cmakeDir = $null
$cmake = Get-Command cmake -ErrorAction SilentlyContinue
if ($cmake) {
    $cmakeDir = Split-Path -Parent $cmake.Source
} else {
    foreach ($candidate in @('C:\Program Files\CMake\bin', (Join-Path $local 'python\cmake\data\bin'))) {
        if (Test-Path -LiteralPath (Join-Path $candidate 'cmake.exe')) { $cmakeDir = $candidate; break }
    }
}
if (-not $cmakeDir) { $missing += 'CMake: winget install Kitware.CMake' }
if (-not (Get-Command tar -ErrorAction SilentlyContinue)) { $missing += 'tar.exe (included with Windows 10 1803 and later)' }
if ($missing.Count -gt 0) {
    Write-Host 'Missing prerequisites. Install them, open a new terminal, then run this script again:' -ForegroundColor Yellow
    foreach ($item in $missing) { Write-Host "  - $item" }
    exit 1
}

# 2. Pinned native dependencies in .local (ignored by Git).
New-Item -ItemType Directory -Force -Path $local | Out-Null
if (Test-Path -LiteralPath (Join-Path $ffmpegDir 'lib\avcodec.lib')) {
    Write-Step "FFmpeg development files already present: $ffmpegDir"
} elseif (Test-Path -LiteralPath $ffmpegDir) {
    throw "$ffmpegDir exists but is incomplete. Delete it and run this script again."
} else {
    $archive = Join-Path $local "$ffmpegName.zip"
    if (-not (Test-Path -LiteralPath $archive) -or (Get-Sha256 $archive) -ne $ffmpegSha256) {
        Save-Verified $ffmpegUrl $ffmpegSha256 $archive
    }
    Write-Step "Extracting $ffmpegName"
    $staging = Join-Path $local 'ffmpeg-extract'
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
    New-Item -ItemType Directory -Path $staging | Out-Null
    & tar -xf $archive -C $staging
    if ($LASTEXITCODE -ne 0) { throw 'Extracting the FFmpeg archive failed.' }
    Move-Item -LiteralPath (Join-Path $staging $ffmpegName) -Destination $ffmpegDir
    Remove-Item -LiteralPath $staging -Recurse -Force
    Remove-Item -LiteralPath $archive -Force
}
if ($NoSpeechModel) {
    Write-Step 'Skipping the Whisper model (-NoSpeechModel)'
} elseif ((Test-Path -LiteralPath $modelPath) -and (Get-Sha256 $modelPath) -eq $modelSha256) {
    Write-Step "Whisper model already present: $modelPath"
} else {
    Save-Verified $modelUrl $modelSha256 $modelPath
}

# 3. Optimized build. build-release.ps1 also verifies native Whisper optimization flags.
if (-not $SkipBuild) {
    $saved = @{}
    foreach ($name in 'FFMPEG_DIR', 'LIBCLANG_PATH', 'PATH') { $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
    try {
        $env:FFMPEG_DIR = $ffmpegDir
        $env:LIBCLANG_PATH = $libclang
        $env:PATH = "$ffmpegDir\bin;$cmakeDir;$env:PATH"
        Write-Step 'Building the release server (the first build takes 10-20 minutes)'
        & (Join-Path $PSScriptRoot 'build-release.ps1') -TargetDirectory $TargetDirectory
    } finally {
        foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
    }
}
if (-not (Test-Path -LiteralPath $exe)) { throw "Server executable not found: $exe. Run this script without -SkipBuild." }

# 4. Start the server the way an agent will, with only the registered environment.
$serverEnvironment = [ordered]@{ PATH = "$ffmpegDir\bin;$env:SystemRoot\System32;$env:SystemRoot" }
if (-not $NoSpeechModel -and (Test-Path -LiteralPath $modelPath)) { $serverEnvironment['VIDEO_WHISPER_MODEL'] = $modelPath }
Write-Step 'Checking that the server starts and lists its tools'
$tools = Test-VideoServer $exe $serverEnvironment
if ($tools -ne 6) { throw "Expected 6 tools, the server listed $tools." }

# 5. Registration commands with this machine's paths.
$codexEnv = ($serverEnvironment.Keys | ForEach-Object { "--env `"$_=$($serverEnvironment[$_])`"" }) -join ' '
$claudeEnv = ($serverEnvironment.Keys | ForEach-Object { "-e `"$_=$($serverEnvironment[$_])`"" }) -join ' '
# Windows PowerShell's ConvertTo-Json indents oddly; lay out the block by hand and let
# it escape only the string values.
$jsonEnv = ($serverEnvironment.Keys | ForEach-Object { "        `"$_`": $($serverEnvironment[$_] | ConvertTo-Json)" }) -join ",`n"
$json = @"
{
  "mcpServers": {
    "video-local": {
      "command": $($exe | ConvertTo-Json),
      "args": [],
      "env": {
$jsonEnv
      }
    }
  }
}
"@
Write-Host ''
Write-Host "Ready: $exe ($tools tools)" -ForegroundColor Green
Write-Host ''
Write-Host 'Register it with your agent, then restart the agent:'
Write-Host ''
Write-Host 'Codex:'
Write-Host "  codex mcp add video-local $codexEnv -- `"$exe`""
Write-Host ''
Write-Host 'Claude Code:'
Write-Host "  claude mcp add video-local -s user $claudeEnv -- `"$exe`""
Write-Host ''
Write-Host 'Claude Desktop (%APPDATA%\Claude\claude_desktop_config.json), Cursor (~/.cursor/mcp.json) and other JSON clients:'
Write-Host $json
Write-Host ''
Write-Host 'Optional: give the agent the video workflow by copying SKILL.md to'
Write-Host '  ~/.codex/skills/understand-video/SKILL.md (Codex) or ~/.claude/skills/understand-video/SKILL.md (Claude Code).'
