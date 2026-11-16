# Glint's update through Scoop. Glint (crates/glint-ui/src/updater/scoop.rs) writes this file and a job
# beside it, then starts it with no window. It waits for Glint to quit, runs `scoop update <app>` the way
# the person would (so Scoop's own record of the version stays right), writes how it went to the job's
# result file, and starts Glint again when the job asks for it.
# ASCII only: Windows PowerShell reads a script without a byte order mark in the ANSI code page.
param([Parameter(Mandatory = $true)][string]$Job)

$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'
$work = Get-Content -LiteralPath $Job -Raw -Encoding UTF8 | ConvertFrom-Json
$current = Join-Path $work.appDir 'current'
$said = New-Object System.Collections.Generic.List[string]

# The version Scoop says is installed (scoop-manifest.json since Scoop 0.6, manifest.json before).
function Get-Installed {
    foreach ($name in 'scoop-manifest.json', 'manifest.json') {
        $path = Join-Path $current $name
        if (Test-Path -LiteralPath $path) {
            try { return [string](Get-Content -LiteralPath $path -Raw -Encoding UTF8 | ConvertFrom-Json).version } catch { }
        }
    }
    return $null
}

function Test-AtLeast([string]$version, [string]$target) {
    if (-not $version) { return $false }
    try { return [version]$version -ge [version]$target } catch { return $version -eq $target }
}

# Runs Scoop in a PowerShell of its own (its scripts may `exit`), for ten minutes at most, keeping what it said.
function Invoke-Scoop([string[]]$arguments) {
    $out = [System.IO.Path]::GetTempFileName()
    $err = [System.IO.Path]::GetTempFileName()
    try {
        $list = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', ('"{0}"' -f $work.scoop)) + $arguments
        $process = Start-Process -FilePath (Join-Path $PSHOME 'powershell.exe') -ArgumentList $list -NoNewWindow -PassThru `
            -RedirectStandardOutput $out -RedirectStandardError $err
        if (-not $process.WaitForExit(600000)) {
            try { $process.Kill() } catch { }
            $said.Add('Scoop took longer than ten minutes and was stopped.')
        }
        foreach ($file in $out, $err) {
            Get-Content -LiteralPath $file -ErrorAction SilentlyContinue | ForEach-Object { $said.Add([string]$_) }
        }
    } catch {
        $said.Add([string]$_)
    } finally {
        Remove-Item -LiteralPath $out, $err -Force -ErrorAction SilentlyContinue
    }
}

$before = Get-Installed

# Scoop won't update an app while it runs from its folder: Glint first, then whatever of it is still ending.
if ($work.waitFor -gt 0) {
    Wait-Process -Id $work.waitFor -Timeout 120 -ErrorAction SilentlyContinue
}
$prefix = $work.appDir.TrimEnd('\') + '\'
$deadline = (Get-Date).AddSeconds(60)
while ((Get-Date) -lt $deadline) {
    $running = Get-Process -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase) }
    if (-not $running) { break }
    Start-Sleep -Milliseconds 500
}

if (-not (Test-AtLeast $before $work.target)) {
    # The bucket on this PC may not know the version yet: Scoop reads its buckets again first.
    $known = $null
    if ($work.bucketManifest -and (Test-Path -LiteralPath $work.bucketManifest)) {
        try { $known = [string](Get-Content -LiteralPath $work.bucketManifest -Raw -Encoding UTF8 | ConvertFrom-Json).version } catch { }
    }
    if (-not (Test-AtLeast $known $work.target)) { Invoke-Scoop @('update') }
    Invoke-Scoop @('update', $work.app)
}

$after = Get-Installed
$ok = Test-AtLeast $after $work.target
$message = (($said | Where-Object { $_ -and $_.Trim() } | Select-Object -Last 8) -join "`n")
[ordered]@{ target = $work.target; before = $before; after = $after; ok = $ok; message = $message } |
    ConvertTo-Json | Set-Content -LiteralPath $work.result -Encoding UTF8
Remove-Item -LiteralPath $Job -Force -ErrorAction SilentlyContinue

if ($work.relaunch) {
    $relaunchArgs = @($work.relaunchArgs | Where-Object { $_ })
    if ($relaunchArgs.Count -gt 0) {
        Start-Process -FilePath $work.relaunch -ArgumentList $relaunchArgs
    } else {
        Start-Process -FilePath $work.relaunch
    }
}
