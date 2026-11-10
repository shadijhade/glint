<#
.SYNOPSIS
  Starts the debug Glint.exe on a scratch folder, optionally does some steps, and prints the UI Automation tree.

.DESCRIPTION
  The tool for writing a new check: shows the accessible names, roles and positions the check can find, after the
  keys or clicks it would do. It backs up and restores the user's state exactly as run.ps1 does.

  -Steps takes a list of steps, done in order before the dump:
    ctrl+shift+f      a key chord (Enter, Escape, F2, alt+left...)
    text:hello        types text
    clickat:x,y       clicks a screen point (rclickat: right-click)
    drag:A>B          drags element A onto element B (sdrag: holding Shift)
    capture:<file>    saves a picture of the main window as a PNG
    click:Name        clicks the first element called Name (exact); rclick:Name right-clicks; dclick:Name double-clicks
    wait:1.5          waits seconds

.EXAMPLE
  powershell -STA -File tests\e2e\dump-ui.ps1 -Steps 'rclick:alpha.txt'
#>
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\..\target\ui\debug\Glint.exe'),
    [string[]]$Steps = @(),
    [switch]$Keep
)

if ($PSVersionTable.PSEdition -eq 'Core' -or [Threading.Thread]::CurrentThread.GetApartmentState() -ne 'STA') {
    $forward = @('-STA', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $PSCommandPath, '-Exe', $Exe)
    if ($Steps.Count) { $forward += '-Steps'; $forward += ($Steps -join '|||') }
    if ($Keep) { $forward += '-Keep' }
    & "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" @forward
    exit $LASTEXITCODE
}
if ($Steps.Count -eq 1 -and $Steps[0].Contains('|||')) { $Steps = $Steps[0] -split '\|\|\|' }

. (Join-Path $PSScriptRoot 'lib\Harness.ps1')

try {
    Open-Session -Exe $Exe
    foreach ($step in $Steps) {
        Write-Log "step: $step"
        if ($step -match '^text:(.*)$') { Send-Text $Matches[1] }
        elseif ($step -match '^capture:(.*)$') { Write-Log ('  capture: ' + [GlintE2E.Win]::Capture((Get-MainWindow).Handle, $Matches[1])) }
        elseif ($step -match '^(clickat|rclickat):(\d+),(\d+)$') { Set-Foreground; Invoke-ClickAt ([int]$Matches[2]) ([int]$Matches[3]) -Right:($Matches[1] -eq 'rclickat') }
        elseif ($step -match '^(drag|sdrag):(.*)>(.*)$') {
            $from = Find-El -Name $Matches[2]; $to = Find-El -Name $Matches[3]
            if (-not $from -or -not $to) { Write-Log '  drag: an element is missing'; continue }
            Invoke-Drag $from $to -Shift:($Matches[1] -eq 'sdrag')
        }
        elseif ($step -match '^wait:(.*)$') { Start-Sleep -Milliseconds ([int]([double]$Matches[1] * 1000)) }
        elseif ($step -match '^(click|rclick|dclick):(.*)$') {
            $el = Find-El -Name $Matches[2]
            if (-not $el) { Write-Log "  no element named '$($Matches[2])'"; continue }
            switch ($Matches[1]) {
                'click' { Invoke-Click $el }
                'rclick' { Invoke-Click $el -Right }
                'dclick' { Invoke-Click $el -Double }
            }
        }
        else { Send-Key $step }
        Start-Sleep -Milliseconds 600
    }
    foreach ($w in (Get-AppWindows)) {
        Write-Host ("=== window '{0}' [{1},{2} {3}x{4}] ===" -f $w.Title, $w.Left, $w.Top, ($w.Right - $w.Left), ($w.Bottom - $w.Top))
        Format-Tree $w | ForEach-Object { Write-Host $_ }
    }
}
finally {
    Close-Session -Keep:$Keep
}
