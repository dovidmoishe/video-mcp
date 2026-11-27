param([switch]$RebuildWhisper, [string]$TargetDirectory = 'target')
$ErrorActionPreference = 'Stop'
$videoWorkspace = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$videoTarget = [System.IO.Path]::GetFullPath((Join-Path $videoWorkspace $TargetDirectory))
if (-not $videoTarget.StartsWith($videoWorkspace + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unexpected Cargo target directory' }
if ((Test-Path -LiteralPath $videoTarget) -and (Get-Item -LiteralPath $videoTarget).Attributes.HasFlag([System.IO.FileAttributes]::ReparsePoint)) { throw 'Refusing cleanup through a redirected target directory' }
$videoPreviousC = $env:CMAKE_C_FLAGS_RELEASE
$videoPreviousCxx = $env:CMAKE_CXX_FLAGS_RELEASE
$videoPreviousTarget = $env:CARGO_TARGET_DIR
Push-Location -LiteralPath $videoWorkspace
try {
    $env:CARGO_TARGET_DIR = $videoTarget
    # The native build can overwrite MSVC Release flags with cc baseline flags,
    # dropping /O2. Explicit CMake definitions preserve native optimization.
    $env:CMAKE_C_FLAGS_RELEASE = '/MD /O2 /Ob2 /DNDEBUG'
    $env:CMAKE_CXX_FLAGS_RELEASE = '/MD /O2 /Ob2 /DNDEBUG'
    $videoCaches = Get-ChildItem -Path (Join-Path $videoTarget 'release/build/whisper-rs-sys-*/out/build/CMakeCache.txt') -ErrorAction SilentlyContinue
    $videoUnoptimized = $false
    foreach ($videoCache in $videoCaches) {
        foreach ($videoFlag in @('CMAKE_C_FLAGS_RELEASE','CMAKE_CXX_FLAGS_RELEASE')) {
            $videoLine = Select-String -LiteralPath $videoCache.FullName -Pattern ('^' + $videoFlag + ':STRING=')
            if (-not $videoLine -or $videoLine.Line -notmatch '/O2') { $videoUnoptimized = $true }
        }
    }
    # Cargo reports progress on stderr. When output is redirected, Windows PowerShell
    # 5.1 turns that into error records that 'Stop' makes fatal; use exit codes instead.
    $ErrorActionPreference = 'Continue'
    if ($RebuildWhisper -or $videoUnoptimized) {
        & cargo clean -p whisper-rs-sys --release
        if ($LASTEXITCODE -ne 0) { throw 'Failed to clear the unoptimized native build' }
    }
    & cargo build --release -p video-mcp
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    $ErrorActionPreference = 'Stop'
    $videoCaches = Get-ChildItem -Path (Join-Path $videoTarget 'release/build/whisper-rs-sys-*/out/build/CMakeCache.txt') -ErrorAction Stop
    foreach ($videoCache in $videoCaches) {
        foreach ($videoFlag in @('CMAKE_C_FLAGS_RELEASE','CMAKE_CXX_FLAGS_RELEASE')) {
            $videoLine = Select-String -LiteralPath $videoCache.FullName -Pattern ('^' + $videoFlag + ':STRING=')
            if (-not $videoLine -or $videoLine.Line -notmatch '/O2') { throw "Native optimization check failed: $($videoCache.FullName)" }
        }
    }
    Write-Output 'Release build passed; native Whisper C/C++ Release flags include /O2'
} finally {
    $env:CMAKE_C_FLAGS_RELEASE = $videoPreviousC
    $env:CMAKE_CXX_FLAGS_RELEASE = $videoPreviousCxx
    $env:CARGO_TARGET_DIR = $videoPreviousTarget
    Pop-Location
}
