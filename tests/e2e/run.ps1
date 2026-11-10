<#
.SYNOPSIS
  End-to-end checks of the real Glint app: starts the debug Glint.exe on a scratch folder and drives it with real
  mouse and keyboard input, reading the result back through UI Automation and the disk.

.PARAMETER Exe
  The Glint.exe to test. Default: target\ui\debug\Glint.exe (cargo build -p glint).

.PARAMETER Only
  Run only the named checks (see -List).

.PARAMETER List
  Print the checks and what each proves, and exit.

.PARAMETER Keep
  Keep the scratch folder (with its log) even when every check passed.

.PARAMETER Restore
  Put back the user's state files from the newest backup that was never restored (after a run that was killed),
  and exit. The clipboard is not backed up on disk.

.EXAMPLE
  powershell -STA -File tests\e2e\run.ps1
  powershell -STA -File tests\e2e\run.ps1 -Only drag-copy,drag-move-shift
#>
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\..\target\ui\debug\Glint.exe'),
    [string[]]$Only = @(),
    [switch]$List,
    [switch]$Keep,
    [switch]$Restore
)

# Windows PowerShell 5.1 in a single-threaded apartment: the clipboard and UI Automation need it.
if ($PSVersionTable.PSEdition -eq 'Core' -or [Threading.Thread]::CurrentThread.GetApartmentState() -ne 'STA') {
    $forward = @('-STA', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $PSCommandPath, '-Exe', $Exe)
    if ($Only.Count) { $forward += '-Only'; $forward += ($Only -join ',') }
    if ($List) { $forward += '-List' }
    if ($Keep) { $forward += '-Keep' }
    if ($Restore) { $forward += '-Restore' }
    & "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" @forward
    exit $LASTEXITCODE
}
if ($Only.Count -eq 1 -and $Only[0].Contains(',')) { $Only = $Only[0] -split ',' }

. (Join-Path $PSScriptRoot 'lib\Harness.ps1')
. (Join-Path $PSScriptRoot 'lib\App.ps1')
. (Join-Path $PSScriptRoot 'checks\Checks.ps1')

if ($List) {
    foreach ($c in $script:Checks) { Write-Host ('{0,-20} {1}' -f $c.Name, $c.Proves) }
    exit 0
}

if ($Restore) {
    $root = Get-BackupRoot
    $dirs = @()
    if (Test-Path -LiteralPath $root) {
        $dirs = @(Get-ChildItem -LiteralPath $root -Directory | Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'manifest.json') } | Sort-Object Name -Descending)
    }
    if ($dirs.Count -eq 0) { Write-Host 'No unrestored backup found.'; exit 0 }
    if (@(Test-OtherGlintRunning).Count -gt 0) { Write-Host 'Glint is running; close it first (it would overwrite the files again).'; exit 1 }
    [void](Restore-UserStateFrom $dirs[0].FullName)
    Write-Host "Put back the state files saved in $($dirs[0].FullName)."
    exit 0
}

foreach ($name in $Only) {
    if (-not ($script:Checks | Where-Object { $_.Name -eq $name })) { Write-Host "No check called '$name' (see -List)."; exit 2 }
}

# What the user's files look like before, to prove they are the same after.
function Get-FileHashes {
    $h = @{}
    foreach ($f in (Get-UserFiles)) {
        if (Test-Path -LiteralPath $f) { $h[$f] = (Get-FileHash -LiteralPath $f -Algorithm SHA256).Hash } else { $h[$f] = '(absent)' }
    }
    return $h
}

$exit = 0
$hashesBefore = Get-FileHashes
$failed = $false
$kept = $null
try {
    Open-Session -Exe $Exe
    # The first window of a fresh process takes a moment to answer clicks; the checks wait for what they need.
    Invoke-Checks -Only $Only -Reset { Reset-App }
}
catch {
    Write-Log ("run aborted: " + $_.Exception.Message)
    $failed = $true
    $exit = 3
}
finally {
    $anyFail = @($script:Results | Where-Object { $_.Status -eq 'FAIL' }).Count -gt 0
    if ($anyFail) { $failed = $true }
    $scratch = $script:E2E.Scratch
    Close-Session -Keep:($Keep -or $failed)
    $hashesAfter = Get-FileHashes
    $restored = $true
    foreach ($k in $hashesBefore.Keys) { if ($hashesBefore[$k] -ne $hashesAfter[$k]) { $restored = $false; Write-Host "NOT RESTORED: $k" } }
    $stray = @(Test-OtherGlintRunning)
    Write-Host ''
    Write-Host '==== results ===='
    foreach ($r in $script:Results) {
        Write-Host ('{0,-5} {1,-20} {2,5}s  {3}' -f $r.Status, $r.Name, $r.Seconds, $r.Detail)
    }
    $pass = @($script:Results | Where-Object { $_.Status -eq 'PASS' }).Count
    $fail = @($script:Results | Where-Object { $_.Status -eq 'FAIL' }).Count
    $skip = @($script:Results | Where-Object { $_.Status -eq 'SKIP' }).Count
    $known = @($script:Results | Where-Object { $_.Status -eq 'KNOWN' }).Count
    $fixed = @($script:Results | Where-Object { $_.Status -eq 'FIXED' }).Count
    Write-Host ("{0} passed, {1} failed, {2} skipped, {3} known issues, {4} fixed (unflag)" -f $pass, $fail, $skip, $known, $fixed)
    Write-Host ("user's state files put back: " + $(if ($restored) { 'yes (hashes match)' } else { 'NO' }))
    Write-Host ("clipboard put back: " + $(if ($script:ClipboardRestored -eq $true) { 'yes (text, files and image kind match)' } elseif ($script:ClipboardRestored -eq $false) { 'NO (it held a format the run cannot copy; see the log)' } else { 'not backed up' }))
    if ($stray.Count -gt 0) { Write-Host "a Glint.exe is still running (pid $($stray[0].Id))" }
    if ($failed -or $Keep) { Write-Host "scratch folder with the log and captures: $scratch" }
    if ($fail -gt 0) { $exit = 1 }
    if (-not $restored -or $stray.Count -gt 0) { $exit = 4 }
}
exit $exit
