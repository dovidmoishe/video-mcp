<#
One-line installer for Windows 10/11 (x64):

    irm https://raw.githubusercontent.com/dovidmoishe/video-mcp/main/scripts/install.ps1 | iex

Downloads the source into %LOCALAPPDATA%\video-mcp (a short path, which whisper.cpp's
MSBuild build needs) and runs scripts\setup.ps1 there: prerequisite check, pinned
FFmpeg and Whisper downloads, release build, server self-test, registration commands.
Rerun it to update; downloaded dependencies and the build cache are kept.

Settings (environment variables):
    VIDEO_MCP_REF   branch, tag or commit to install (default: main)
    VIDEO_MCP_HOME  install folder (default: %LOCALAPPDATA%\video-mcp)
#>
& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $ref = if ($env:VIDEO_MCP_REF) { $env:VIDEO_MCP_REF } else { 'main' }
    $root = if ($env:VIDEO_MCP_HOME) { $env:VIDEO_MCP_HOME } else { Join-Path $env:LOCALAPPDATA 'video-mcp' }
    $archive = Join-Path $env:TEMP ('video-mcp-' + [guid]::NewGuid().ToString('N') + '.zip')
    $staging = "$archive.extract"
    try {
        Write-Host "==> Downloading video-mcp ($ref)" -ForegroundColor Cyan
        # zip/<ref> takes a branch, tag or commit; branch names with a slash need the full form.
        $path = if ($ref -match '/') { "refs/heads/$ref" } else { $ref }
        Invoke-WebRequest -Uri "https://codeload.github.com/dovidmoishe/video-mcp/zip/$path" -OutFile $archive -UseBasicParsing
        New-Item -ItemType Directory -Path $staging | Out-Null
        & tar -xf $archive -C $staging
        if ($LASTEXITCODE -ne 0) { throw 'Extracting the source archive failed.' }
        $source = (Get-ChildItem -LiteralPath $staging -Directory | Select-Object -First 1).FullName
        New-Item -ItemType Directory -Force -Path $root | Out-Null
        # Mirror the source, keeping downloaded dependencies (.local) and the build cache (target).
        & robocopy $source $root /MIR /XD .local target /NFL /NDL /NJH /NJS /NP | Out-Null
        if ($LASTEXITCODE -ge 8) { throw "Copying the source failed (robocopy exit code $LASTEXITCODE)." }
    } finally {
        Remove-Item -LiteralPath $archive -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
    }
    Write-Host "==> Source is in $root" -ForegroundColor Cyan
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root 'scripts\setup.ps1')
    if ($LASTEXITCODE -ne 0) {
        Write-Host 'Setup did not finish. Fix the problem it reported, then run the same command again.' -ForegroundColor Yellow
    }
}
