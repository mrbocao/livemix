param(
    [Parameter(Mandatory)][string]$MediaMTX,
    [int]$RtmpPort = 21935,
    [int]$SrtPort = 28890
)
$ErrorActionPreference = 'Stop'
$server = (Resolve-Path -LiteralPath $MediaMTX).Path
$workspace = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $workspace 'app/Cargo.toml'
$runDir = Join-Path ([IO.Path]::GetTempPath()) ('livemix-mediamtx-' + [guid]::NewGuid())
$null = New-Item -ItemType Directory -Path $runDir
$config = Join-Path $runDir 'mediamtx.yml'
$log = Join-Path $runDir 'mediamtx.log'
$process = $null
try {
    [IO.File]::WriteAllText($config, @"
rtsp: no
rtmpAddress: 127.0.0.1:$RtmpPort
hls: no
webrtc: no
srtAddress: 127.0.0.1:$SrtPort
paths:
  all_others:
"@, [Text.UTF8Encoding]::new($false))
    $options = @{
        FilePath = $server
        ArgumentList = ('"' + $config + '"')
        RedirectStandardOutput = $log
        WorkingDirectory = $runDir
        PassThru = $true
    }
    if ($env:OS -eq 'Windows_NT') { $options.WindowStyle = 'Hidden' }
    $process = Start-Process @options
    Start-Sleep -Milliseconds 500
    if ($process.HasExited) { throw "MediaMTX exited with $($process.ExitCode)" }
    $env:LIVEMIX_MEDIAMTX_RTMP_PORT = $RtmpPort
    $env:LIVEMIX_MEDIAMTX_SRT_PORT = $SrtPort
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    $cargoPath = if ($cargo) { $cargo.Source } else { Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
    & $cargoPath test --locked --release --manifest-path $manifest mediamtx_delivers -- --ignored --nocapture
    if ($LASTEXITCODE -ne 0) { throw "Streaming test failed ($LASTEXITCODE)" }
} catch {
    if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log -Tail 40 }
    throw
} finally {
    if ($process -and !$process.HasExited) { Stop-Process -Id $process.Id }
    if ($process) { $process.WaitForExit() }
    foreach ($name in @('mediamtx.yml', 'mediamtx.log', 'auto.key', 'auto.crt')) {
        $file = Join-Path $runDir $name
        if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file }
    }
    if (!(Get-ChildItem -LiteralPath $runDir -Force | Select-Object -First 1)) {
        Remove-Item -LiteralPath $runDir
    }
}
