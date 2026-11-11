# The end-to-end harness: session handling (start the debug Glint.exe on a scratch folder, put everything back
# afterwards), UI Automation helpers (find controls by their accessible names), real input, and the check runner.
# Dot-sourced by run.ps1 and dump-ui.ps1. Windows PowerShell 5.1, STA.

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms, System.Drawing
if (-not ('GlintE2E.Win' -as [type])) {
    Add-Type -Path (Join-Path $PSScriptRoot 'Native.cs') -ReferencedAssemblies System.Drawing, System.Windows.Forms
}
[GlintE2E.Win]::DpiAware()

# Everything the session knows, in one place.
$script:E2E = @{
    Exe        = $null
    Scratch    = $null     # %TEMP%\glint-e2e-<stamp>: the only place the run writes files
    Arena      = $null     # the folder the app browses
    Results    = $null     # logs and captures of failures
    Pid        = 0
    Process    = $null
    Backup     = $null     # copies of the user's state files
    Clipboard  = $null
    Explorer   = [IntPtr]::Zero   # a File Explorer window the run opened (closed again at the end)
    Log        = $null
    EnvSaved   = @{}
}

# ------------------------------------------------------------------------------------------------ logging

function Write-Log {
    param([string]$Message)
    $line = '{0:HH:mm:ss.fff} {1}' -f (Get-Date), $Message
    Write-Host $line
    if ($script:E2E.Log) {
        try { Add-Content -LiteralPath $script:E2E.Log -Value $line -Encoding UTF8 } catch { }
    }
}

# ------------------------------------------------------------------------------------------------ user state

# The files the app reads and writes that belong to the user. The run replaces them with nothing (a fresh
# profile: defaults, no tabs restored, no saved place) and puts the originals back at the end.
function Get-UserFiles {
    $roaming = Join-Path $env:APPDATA 'com.shado.shadui-ripgrep'
    $local = Join-Path $env:LOCALAPPDATA 'com.shado.shadui-ripgrep'
    return @(
        (Join-Path $roaming 'state.json'),
        (Join-Path $local 'window.json'),
        (Join-Path $local 'startup.json')
    )
}

function Get-BackupRoot { return (Join-Path $env:TEMP 'glint-e2e-backups') }

function Backup-UserState {
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $dir = Join-Path (Get-BackupRoot) $stamp
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $entries = @()
    foreach ($file in (Get-UserFiles)) {
        $exists = Test-Path -LiteralPath $file
        if ($exists) { Copy-Item -LiteralPath $file -Destination (Join-Path $dir (Split-Path $file -Leaf)) -Force }
        $entries += @{ path = $file; existed = $exists }
    }
    # The manifest is what `run.ps1 -Restore` reads if this run is killed before it can put things back.
    ConvertTo-Json -InputObject $entries | Set-Content -LiteralPath (Join-Path $dir 'manifest.json') -Encoding UTF8
    $script:E2E.Backup = $dir

    $script:E2E.Clipboard = New-Object GlintE2E.ClipboardBackup
    $script:E2E.Clipboard.Save()
    Write-Log ("backed up the user's state to $dir (clipboard: " + $script:E2E.Clipboard.Summary + ')')
}

function Restore-UserStateFrom {
    param([string]$Dir)
    $manifest = Join-Path $Dir 'manifest.json'
    if (-not (Test-Path -LiteralPath $manifest)) { return $false }
    $entries = Get-Content -LiteralPath $manifest -Raw -Encoding UTF8 | ConvertFrom-Json
    foreach ($entry in @($entries)) {
        $file = [string]$entry.path
        $copy = Join-Path $Dir (Split-Path $file -Leaf)
        if ($entry.existed) {
            New-Item -ItemType Directory -Force -Path (Split-Path $file) | Out-Null
            Copy-Item -LiteralPath $copy -Destination $file -Force
        }
        elseif (Test-Path -LiteralPath $file) {
            Remove-Item -LiteralPath $file -Force
        }
    }
    Move-Item -LiteralPath $manifest -Destination (Join-Path $Dir 'restored.json') -Force
    return $true
}

function Restore-UserState {
    if ($script:E2E.Backup) {
        if (Restore-UserStateFrom $script:E2E.Backup) { Write-Log "put the user's state files back" }
        $script:E2E.Backup = $null
    }
    if ($script:E2E.Clipboard) {
        $script:E2E.Clipboard.Restore()
        $script:ClipboardRestored = $script:E2E.Clipboard.Matches()
        Write-Log ("put the clipboard back (" + $script:E2E.Clipboard.Summary + '); it matches what was there: ' + $script:ClipboardRestored)
        $script:E2E.Clipboard = $null
    }
}

# A fresh profile for the run: no state.json, no startup hints, and a window.json that puts the window at a
# known place and size (physical pixels), so the checks' geometry is the same on every run.
function Set-FreshProfile {
    param([int]$Left, [int]$Top, [int]$Width, [int]$Height)
    foreach ($file in (Get-UserFiles)) {
        if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file -Force }
    }
    $window = Get-UserFiles | Where-Object { $_ -like '*window.json' }
    New-Item -ItemType Directory -Force -Path (Split-Path $window) | Out-Null
    $json = '{{"left":{0},"top":{1},"right":{2},"bottom":{3},"maximized":false}}' -f $Left, $Top, ($Left + $Width), ($Top + $Height)
    Set-Content -LiteralPath $window -Value $json -Encoding ASCII
}

# ------------------------------------------------------------------------------------------------ scratch folder

function Assert-InScratch {
    param([string]$Path)
    $full = [IO.Path]::GetFullPath($Path)
    $root = [IO.Path]::GetFullPath($script:E2E.Scratch).TrimEnd('\') + '\'
    if (-not $full.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to touch ${full}: it is outside the scratch folder ${root}"
    }
}

function New-Scratch {
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $scratch = Join-Path $env:TEMP "glint-e2e-$stamp"
    New-Item -ItemType Directory -Force -Path $scratch | Out-Null
    $script:E2E.Scratch = $scratch
    $script:E2E.Arena = Join-Path $scratch 'arena'
    $script:E2E.Results = Join-Path $scratch 'results'
    New-Item -ItemType Directory -Force -Path $script:E2E.Results | Out-Null
    $script:E2E.Log = Join-Path $script:E2E.Results 'run.log'
    New-Item -ItemType Directory -Force -Path (Join-Path $scratch 'from-explorer') | Out-Null
    Set-Content -LiteralPath (Join-Path $scratch 'from-explorer\fromexplorer.txt') -Value 'dropped from File Explorer' -Encoding UTF8
    Write-Log "scratch folder: $scratch"
}

$script:ArabicName = [string]([char]0x0645) + [char]0x0644 + [char]0x0641 + '.txt'   # Arabic: "mlf.txt" (meem, lam, feh)

# The arena as every check expects to find it. Files are written without a byte order mark.
function Reset-Arena {
    $arena = $script:E2E.Arena
    Assert-InScratch $arena
    New-Item -ItemType Directory -Force -Path $arena | Out-Null
    Get-ChildItem -LiteralPath $arena -Force | ForEach-Object {
        Assert-InScratch $_.FullName
        Remove-Item -LiteralPath $_.FullName -Recurse -Force
    }
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    foreach ($dir in 'inbox', 'target', 'target2') { New-Item -ItemType Directory -Force -Path (Join-Path $arena $dir) | Out-Null }
    [IO.File]::WriteAllText((Join-Path $arena 'inbox\inner.txt'), "inner file`r`n", $utf8)
    [IO.File]::WriteAllText((Join-Path $arena 'alpha.txt'), "alpha line one`r`nthe needle is in this file`r`nalpha line three`r`n", $utf8)
    [IO.File]::WriteAllText((Join-Path $arena 'beta.txt'), "beta`r`n", $utf8)
    [IO.File]::WriteAllText((Join-Path $arena 'dragme.txt'), "copy me by dragging`r`n", $utf8)
    [IO.File]::WriteAllText((Join-Path $arena 'moveme.txt'), "move me with Shift`r`n", $utf8)
    [IO.File]::WriteAllText((Join-Path $arena 'gamma.md'), "# Gamma`r`n`r`nA small Markdown file.`r`n", $utf8)
    [IO.File]::WriteAllText((Join-Path $arena $script:ArabicName), "arabic name`r`n", $utf8)
}

# What the listing should show, in the order the app shows it (folders first).
function Get-ArenaNames {
    $names = New-Object System.Collections.ArrayList
    foreach ($n in 'inbox', 'target', 'target2', 'alpha.txt', 'beta.txt', 'dragme.txt', 'gamma.md', 'moveme.txt') { [void]$names.Add($n) }
    [void]$names.Add($script:ArabicName)
    return $names.ToArray()
}

# ------------------------------------------------------------------------------------------------ the app

function Test-OtherGlintRunning {
    return @(Get-Process -Name 'Glint' -ErrorAction SilentlyContinue)
}

function Set-ChildEnv {
    # The debug app reads these; the run pins them so the names are English and the theme is fixed.
    $want = @{ GLINT_LANGUAGE = 'en'; GLINT_THEME = 'light'; GLINT_SELFTEST = $null; GLINT_BENCH_START = $null }
    foreach ($key in $want.Keys) {
        $script:E2E.EnvSaved[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
        [Environment]::SetEnvironmentVariable($key, $want[$key], 'Process')
    }
}

function Restore-ChildEnv {
    foreach ($key in @($script:E2E.EnvSaved.Keys)) {
        [Environment]::SetEnvironmentVariable($key, $script:E2E.EnvSaved[$key], 'Process')
    }
    $script:E2E.EnvSaved = @{}
}

function Start-App {
    param([string]$Folder)
    $exe = $script:E2E.Exe
    $others = @(Test-OtherGlintRunning)
    if ($others.Count -gt 0) {
        throw ("Another Glint.exe is already running (pid " + (($others | ForEach-Object { $_.Id }) -join ', ') +
            "): a second launch would hand the folder to it. Close it and run again.")
    }
    Set-ChildEnv
    try {
        $p = Start-Process -FilePath $exe -ArgumentList ('"' + $Folder + '"') -PassThru
    }
    finally { Restore-ChildEnv }
    $script:E2E.Process = $p
    $script:E2E.Pid = $p.Id
    Write-Log "started $exe (pid $($p.Id)) on $Folder"
    $found = Wait-Until -Seconds 30 -What 'the main window' -Cond { $null -ne (Get-MainWindow) }
    if (-not $found) { throw 'Glint did not show its main window within 30 s' }
    Start-Sleep -Milliseconds 700
}

function Stop-App {
    $p = $script:E2E.Process
    if (-not $p) { return }
    try {
        $p.Refresh()
        if (-not $p.HasExited) {
            foreach ($w in [GlintE2E.Win]::TopWindows($p.Id)) {
                if ($w.Visible -and $w.Class -eq 'Zed::Window') { [GlintE2E.Win]::Close($w.Handle) }
            }
            if (-not $p.WaitForExit(8000)) {
                Write-Log 'Glint did not quit when asked; killing it'
                $p.Kill()
                [void]$p.WaitForExit(5000)
            }
        }
        Write-Log "Glint exited with code $($p.ExitCode)"
    }
    catch { Write-Log "while stopping Glint: $($_.Exception.Message)" }
    $script:E2E.Process = $null
}

# The app's visible top-level windows (main window, Settings, Quick Search...).
function Get-AppWindows {
    if (-not $script:E2E.Pid) { return @() }
    return @([GlintE2E.Win]::TopWindows($script:E2E.Pid) | Where-Object { $_.Visible -and $_.Class -eq 'Zed::Window' -and ($_.Right - $_.Left) -gt 50 })
}

function Get-MainWindow {
    $wins = Get-AppWindows
    foreach ($w in $wins) { if ($w.Title -eq 'Glint') { return $w } }
    return $null
}

function Get-Root {
    param($Window)
    if (-not $Window) { $Window = Get-MainWindow }
    if (-not $Window) { throw 'the Glint window is gone' }
    return [System.Windows.Automation.AutomationElement]::FromHandle($Window.Handle)
}

# ------------------------------------------------------------------------------------------------ waiting

# Polls `$Cond` (exceptions count as "not yet": a stale UI Automation element throws). True once it held.
function Wait-Until {
    param([scriptblock]$Cond, [double]$Seconds = 6, [string]$What = '')
    $end = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ($true) {
        $ok = $false
        try { $ok = [bool](& $Cond) } catch { $ok = $false }
        if ($ok) { return $true }
        if ([DateTime]::UtcNow -ge $end) { return $false }
        Start-Sleep -Milliseconds 100
    }
}

function Assert-Eventually {
    param([scriptblock]$Cond, [string]$Message, [double]$Seconds = 6)
    if (-not (Wait-Until -Cond $Cond -Seconds $Seconds)) { throw "Timed out waiting for: $Message" }
}

function Assert-True {
    param($Value, [string]$Message)
    if (-not $Value) { throw "Assertion failed: $Message" }
}

function Assert-Equal {
    param($Actual, $Expected, [string]$Message)
    $a = ($Actual | ForEach-Object { "$_" }) -join ' | '
    $e = ($Expected | ForEach-Object { "$_" }) -join ' | '
    if ($a -ne $e) { throw "Assertion failed: $Message`n  expected: $e`n  actual:   $a" }
}

# ------------------------------------------------------------------------------------------------ UI Automation

function ConvertTo-ControlType {
    param([string]$Type)
    $ct = [System.Windows.Automation.ControlType]
    return $ct::$Type
}

# All descendants of `$Root` (the main window by default) with the given control type and, when given, name.
# Names match exactly (case-insensitive) unless `-Like` asks for a wildcard match.
function Find-Els {
    param([string]$Type, [string]$Name, [switch]$Like, $Root)
    if (-not $Root) { $Root = Get-Root }
    if ($Type) {
        $cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, (ConvertTo-ControlType $Type))
    }
    else { $cond = [System.Windows.Automation.Condition]::TrueCondition }
    $all = $Root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $cond)
    $out = New-Object System.Collections.ArrayList
    foreach ($e in $all) {
        if ($PSBoundParameters.ContainsKey('Name')) {
            $n = $e.Current.Name
            if ($Like) { if ($n -notlike $Name) { continue } }
            elseif ($n -ne $Name) { continue }
        }
        [void]$out.Add($e)
    }
    return $out.ToArray()
}

function Find-El {
    param([string]$Type, [string]$Name, [switch]$Like, $Root)
    $args2 = @{}
    if ($Type) { $args2.Type = $Type }
    if ($PSBoundParameters.ContainsKey('Name')) { $args2.Name = $Name }
    if ($Like) { $args2.Like = $true }
    if ($Root) { $args2.Root = $Root }
    $all = @(Find-Els @args2)
    if ($all.Count -eq 0) { return $null }
    return $all[0]
}

function Get-ElName { param($El) return $El.Current.Name }

# A rectangle's number as an int; an element that is not on screen has an empty rectangle with infinite sides.
function ConvertTo-Pixel {
    param($Value)
    $d = [double]$Value
    if ([double]::IsNaN($d) -or [double]::IsInfinity($d)) { return 0 }
    return [int][Math]::Round($d)
}

function Get-Rect {
    param($El)
    $r = $El.Current.BoundingRectangle
    $l = ConvertTo-Pixel $r.Left
    $t = ConvertTo-Pixel $r.Top
    $w = ConvertTo-Pixel $r.Width
    $h = ConvertTo-Pixel $r.Height
    return @{ L = $l; T = $t; W = $w; H = $h; R = ($l + $w); B = ($t + $h) }
}

# A screen point inside the element: its centre, or `-FromStart` pixels in from the start edge (rows are wide,
# and their middle is empty space; dragging from there would start a marquee instead of carrying the file).
function Get-Point {
    param($El, [int]$FromStart = -1)
    $r = Get-Rect $El
    if ($r.W -le 0 -or $r.H -le 0) { throw "'$($El.Current.Name)' has no size on screen" }
    $x = $r.L + [int]($r.W / 2)
    if ($FromStart -ge 0) { $x = $r.L + [Math]::Min($FromStart, $r.W - 2) }
    return @{ X = $x; Y = $r.T + [int]($r.H / 2) }
}

# The text value of an edit box (what it holds), or ''.
function Get-ElText {
    param($El)
    try {
        $p = $El.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
        return [string]$p.Current.Value
    }
    catch { return '' }
}

function Get-IsSelected {
    param($El)
    try {
        $p = $El.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern)
        return [bool]$p.Current.IsSelected
    }
    catch { return $false }
}

# The file lists on screen (two with the dual pane): each is the 'Files' (or 'Search results'...) list.
function Get-FileLists {
    $out = @()
    foreach ($name in 'Files', 'Search results', 'Recycle Bin') { $out += @(Find-Els -Type List -Name $name) }
    return $out
}

function Get-FileList {
    param([int]$Index = 0)
    $lists = @(Get-FileLists)
    if ($lists.Count -le $Index) { return $null }
    return $lists[$Index]
}

# Rows of a file list as @{ Name; El; Selected }.
function Get-Rows {
    param([int]$List = 0)
    $listEl = Get-FileList $List
    if (-not $listEl) { return @() }
    $rows = @()
    foreach ($e in (Find-Els -Type ListItem -Root $listEl)) {
        $rows += [pscustomobject]@{ Name = $e.Current.Name; El = $e; Selected = (Get-IsSelected $e) }
    }
    return $rows
}

function Get-RowNames {
    param([int]$List = 0)
    return @(Get-Rows -List $List | ForEach-Object { $_.Name })
}

function Get-Row {
    param([string]$Name, [int]$List = 0)
    foreach ($r in (Get-Rows -List $List)) { if ($r.Name -eq $Name) { return $r.El } }
    return $null
}

function Get-SelectedNames {
    param([int]$List = 0)
    return @(Get-Rows -List $List | Where-Object { $_.Selected } | ForEach-Object { $_.Name })
}

# The active tab's title: the folder's name.
function Get-TabTitle {
    $tabs = @(Find-Els -Type TabItem)
    foreach ($t in $tabs) {
        try {
            $p = $t.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern)
            if ($p.Current.IsSelected) { return $t.Current.Name }
        }
        catch { }
    }
    if ($tabs.Count -gt 0) { return $tabs[0].Current.Name }
    return ''
}

# A one-line-per-element dump of the window, for the failure logs and dump-ui.ps1.
function Format-Tree {
    param($Window, [int]$Max = 600)
    $root = Get-Root $Window
    $lines = @()
    $all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    $i = 0
    foreach ($e in $all) {
        $c = $e.Current
        $r = $c.BoundingRectangle
        $type = $c.ControlType.ProgrammaticName -replace '^ControlType\.', ''
        $extra = ''
        if ($type -eq 'ListItem' -or $type -eq 'TabItem' -or $type -eq 'RadioButton') { if (Get-IsSelected $e) { $extra = ' [selected]' } }
        if ($c.IsOffscreen) { $extra += ' [offscreen]' }
        if (-not $c.IsEnabled) { $extra += ' [disabled]' }
        $quoted = "'" + $c.Name + "'"
        $lines += ('{0,4} {1,-12} {2}  ({3},{4} {5}x{6}){7}' -f $i, $type, $quoted, (ConvertTo-Pixel $r.Left), (ConvertTo-Pixel $r.Top), (ConvertTo-Pixel $r.Width), (ConvertTo-Pixel $r.Height), $extra)
        $i++
        if ($i -ge $Max) { $lines += '  ...'; break }
    }
    return $lines
}

# ------------------------------------------------------------------------------------------------ input

$script:VK = @{
    enter = 0x0D; return = 0x0D; escape = 0x1B; esc = 0x1B; space = 0x20; tab = 0x09; backspace = 0x08; delete = 0x2E; del = 0x2E
    up = 0x26; down = 0x28; left = 0x25; right = 0x27; home = 0x24; end = 0x23; pageup = 0x21; pagedown = 0x22
    ctrl = 0x11; shift = 0x10; alt = 0x12; menu = 0x5D
    ',' = 0xBC; '.' = 0xBE; '/' = 0xBF; '=' = 0xBB; '-' = 0xBD; '`' = 0xC0; ';' = 0xBA
}

function ConvertTo-VKey {
    param([string]$Key)
    $k = $Key.ToLowerInvariant()
    if ($script:VK.ContainsKey($k)) { return [uint16]$script:VK[$k] }
    if ($k -match '^f(\d{1,2})$') { return [uint16](0x70 + [int]$Matches[1] - 1) }
    if ($k.Length -eq 1 -and $k -match '[a-z0-9]') { return [uint16][int][char]$k.ToUpperInvariant() }
    throw "unknown key '$Key'"
}

# Makes sure `$Window` (the main window by default) has the keyboard, since SendInput goes to the foreground one.
function Set-Foreground {
    param($Window)
    if (-not $Window) { $Window = Get-MainWindow }
    if (-not $Window) { throw 'the Glint window is gone' }
    if ([GlintE2E.Win]::Foreground() -eq $Window.Handle) { return }
    if (-not [GlintE2E.Win]::Focus($Window.Handle)) {
        throw 'could not bring the Glint window to the front (Windows refused SetForegroundWindow)'
    }
}

# Presses a chord: 'ctrl+shift+n', 'Enter', 'F2'. Real key events, delivered to the foreground window.
function Send-Key {
    param([string]$Chord, $Window)
    Set-Foreground $Window
    $keys = @($Chord -split '\+(?=.)' | ForEach-Object { ConvertTo-VKey $_ })
    [GlintE2E.Win]::Chord([uint16[]]$keys)
}

function Send-Text {
    param([string]$Text, $Window)
    Set-Foreground $Window
    [GlintE2E.Win]::TypeText($Text)
}

function Invoke-Click {
    param($El, [switch]$Right, [switch]$Double, [int]$FromStart = -1, $Window)
    Set-Foreground $Window
    $pt = Get-Point $El -FromStart $FromStart
    if ($Double) { [GlintE2E.Win]::DoubleClick($pt.X, $pt.Y) }
    else { [GlintE2E.Win]::Click($pt.X, $pt.Y, [bool]$Right) }
}

# A real drag with the left button from one point to another (screen pixels). `-Shift` holds Shift once the drag is
# under way, which is how Glint (as Explorer does) turns a drop into a move.
function Invoke-DragPoints {
    param([int]$X1, [int]$Y1, [int]$X2, [int]$Y2, [switch]$Shift, [int]$Steps = 18, [int]$HoldMs = 500)
    $key = [uint16]0
    if ($Shift) { $key = [uint16]0x10 }
    [GlintE2E.Win]::Drag($X1, $Y1, $X2, $Y2, $Steps, 18, $HoldMs, $key)
}

# Drags one element onto another: from `-FromStart` pixels inside the source (its name, not the empty middle of a
# wide row) to the target's centre.
function Invoke-Drag {
    param($From, $To, [switch]$Shift, [int]$FromStart = 70, [int]$ToStart = 70, $Window)
    Set-Foreground $Window
    $a = Get-Point $From -FromStart $FromStart
    $b = Get-Point $To -FromStart $ToStart
    Invoke-DragPoints -X1 $a.X -Y1 $a.Y -X2 $b.X -Y2 $b.Y -Shift:$Shift
}

function Invoke-ClickAt {
    param([int]$X, [int]$Y, [switch]$Right)
    [GlintE2E.Win]::Click($X, $Y, [bool]$Right)
}

# ------------------------------------------------------------------------------------------------ checks

$script:ClipboardRestored = $null
$script:Checks = New-Object System.Collections.ArrayList
$script:Results = New-Object System.Collections.ArrayList

# `-Known` marks a check that fails today because of a bug in the app (the text says which): the run reports it as
# KNOWN instead of FAIL, and says FIXED when it starts to pass, so the flag gets removed.
# `-EndsApp` marks a check that quits the app (it is the last one).
function Add-Check {
    param([string]$Name, [string]$Proves, [scriptblock]$Body, [string]$Known = '', [switch]$EndsApp)
    [void]$script:Checks.Add([pscustomobject]@{ Name = $Name; Proves = $Proves; Body = $Body; Known = $Known; EndsApp = [bool]$EndsApp })
}

# Captures every window of the app (and any File Explorer window the run opened) into the results folder.
function Save-FailureCaptures {
    param([string]$Check)
    $i = 0
    foreach ($w in (Get-AppWindows)) {
        $file = Join-Path $script:E2E.Results ('{0}-fail-{1}.png' -f $Check, $i)
        $how = [GlintE2E.Win]::Capture($w.Handle, $file)
        if ($how) { Write-Log "  capture ($how): $file" }
        $i++
    }
    if ($script:E2E.Explorer -ne [IntPtr]::Zero -and [GlintE2E.Win]::Exists($script:E2E.Explorer)) {
        $file = Join-Path $script:E2E.Results ('{0}-fail-explorer.png' -f $Check)
        $how = [GlintE2E.Win]::Capture($script:E2E.Explorer, $file)
        if ($how) { Write-Log "  capture ($how): $file" }
    }
    try {
        $main = Get-MainWindow
        if ($main) { Set-Content -LiteralPath (Join-Path $script:E2E.Results "$Check-fail-tree.txt") -Value (Format-Tree $main) -Encoding UTF8 }
    }
    catch { }
}

# Runs the checks (all, or the names in `$Only`), each from a fresh state, and returns the result objects.
function Invoke-Checks {
    param([string[]]$Only, [scriptblock]$Reset)
    foreach ($check in $script:Checks) {
        if ($Only -and $Only.Count -gt 0 -and ($Only -notcontains $check.Name)) { continue }
        Write-Log "---- $($check.Name): $($check.Proves)"
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $status = 'PASS'
        $detail = ''
        try {
            if ($Reset) { & $Reset }
            & $check.Body
        }
        catch {
            $status = 'FAIL'
            $detail = $_.Exception.Message
            if ($_.InvocationInfo -and $_.InvocationInfo.ScriptLineNumber) {
                $detail += "  [" + (Split-Path $_.InvocationInfo.ScriptName -Leaf) + ':' + $_.InvocationInfo.ScriptLineNumber + ']'
            }
            Write-Log "  FAIL: $detail"
            try { Save-FailureCaptures $check.Name } catch { Write-Log "  (no capture: $($_.Exception.Message))" }
        }
        if ($check.Known) {
            if ($status -eq 'FAIL') { $status = 'KNOWN'; $detail = $check.Known + ' | ' + $detail; Write-Log "  KNOWN ISSUE: $($check.Known)" }
            else { $status = 'FIXED'; $detail = 'passes now: remove -Known from this check (was: ' + $check.Known + ')'; Write-Log "  FIXED: $detail" }
        }
        if ($status -eq 'PASS') { Write-Log '  PASS' }
        if ($script:SkipNote) { $status = 'SKIP'; $detail = $script:SkipNote; $script:SkipNote = $null; Write-Log "  SKIP: $detail" }
        [void]$script:Results.Add([pscustomobject]@{ Name = $check.Name; Status = $status; Seconds = [Math]::Round($sw.Elapsed.TotalSeconds, 1); Detail = $detail })
        # An app that quit, crashed or stopped answering ends the run: the checks after it need it alive.
        if ($script:E2E.Process) {
            $script:E2E.Process.Refresh()
            if ($script:E2E.Process.HasExited) {
                if (-not $check.EndsApp) { Write-Log '  the app has exited; the remaining checks cannot run' }
                break
            }
            if (-not $script:E2E.Process.Responding) {
                Write-Log '  the app has stopped answering; the remaining checks cannot run (it is killed at the end)'
                [void]$script:Results.Add([pscustomobject]@{ Name = '(app hung)'; Status = 'FAIL'; Seconds = 0; Detail = "Glint stopped answering after '$($check.Name)'" })
                break
            }
        }
    }
}

# Marks the running check as skipped (it could not run on this PC), with the reason.
function Set-Skip {
    param([string]$Reason)
    $script:SkipNote = $Reason
}
$script:SkipNote = $null

# ------------------------------------------------------------------------------------------------ session

# Closes the File Explorer window the run opened, if any. Only that one window, found by its handle.
function Close-OurExplorer {
    # The window the run opened is known by its handle; as a safety net, any Explorer window whose title shows the
    # scratch folder's own (timestamped) name is ours too, whether or not the handle was recorded.
    if ($script:E2E.Scratch) {
        $stamp = Split-Path $script:E2E.Scratch -Leaf
        foreach ($w in [GlintE2E.Win]::WindowsOfClass('CabinetWClass')) {
            if ($w.Title -like "*$stamp*") { [GlintE2E.Win]::Close($w.Handle) }
        }
    }
    $h = $script:E2E.Explorer
    if ($h -eq [IntPtr]::Zero) { return }
    if ([GlintE2E.Win]::Exists($h)) {
        [GlintE2E.Win]::Close($h)
        $gone = Wait-Until -Seconds 6 -Cond { -not [GlintE2E.Win]::Exists($h) }
        if (-not $gone) { Write-Log 'the File Explorer window did not close; close it by hand' }
    }
    $script:E2E.Explorer = [IntPtr]::Zero
}

# Starts a session: backs up the user's state, makes the scratch folder, starts the app on its arena.
function Open-Session {
    param([string]$Exe)
    if (-not (Test-Path -LiteralPath $Exe)) { throw "No Glint.exe at $Exe (build it: cargo build -p glint)" }
    $script:E2E.Exe = (Resolve-Path -LiteralPath $Exe).Path
    $others = @(Test-OtherGlintRunning)
    if ($others.Count -gt 0) {
        throw ("Another Glint.exe is running (pid " + (($others | ForEach-Object { $_.Id }) -join ', ') + '). Close it first: the run needs the app to itself.')
    }
    # A run that was killed leaves the user's files replaced by the fresh profile and its backup unrestored. Starting
    # another would back up that fresh profile as if it were the user's, and lose the originals: refuse.
    $root = Get-BackupRoot
    if (Test-Path -LiteralPath $root) {
        foreach ($d in (Get-ChildItem -LiteralPath $root -Directory)) {
            if (Test-Path -LiteralPath (Join-Path $d.FullName 'manifest.json')) {
                throw "An earlier run left the user's state files unrestored (backup in $($d.FullName)). Run: powershell -STA -File tests/e2e/run.ps1 -Restore"
            }
            if (Test-Path -LiteralPath (Join-Path $d.FullName 'restored.json')) { Remove-Item -LiteralPath $d.FullName -Recurse -Force }
        }
    }
    New-Scratch
    Backup-UserState
    Reset-Arena
    $work = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
    $w = [Math]::Min(1100, $work.Width - 60)
    # Tall enough that the arena's items leave empty list under them (the checks click and right-click there).
    $h = [Math]::Min(760, $work.Height - 60)
    Set-FreshProfile -Left ($work.Left + 30) -Top ($work.Top + 20) -Width $w -Height $h
    Start-App -Folder $script:E2E.Arena
}

# Ends it: closes what the run opened, quits the app, puts the user's files and clipboard back, and removes the
# scratch folder unless something failed (its results are the evidence) or `-Keep` says to keep it.
function Close-Session {
    param([switch]$Keep)
    try { Close-OurExplorer } catch { Write-Log "explorer cleanup: $($_.Exception.Message)" }
    try { Stop-App } catch { Write-Log "app cleanup: $($_.Exception.Message)" }
    try { Restore-UserState } catch { Write-Log "restore: $($_.Exception.Message)" }
    if ($script:E2E.Scratch -and -not $Keep) {
        $script:E2E.Log = $null
        try {
            Assert-InScratch (Join-Path $script:E2E.Scratch 'x')
            Remove-Item -LiteralPath $script:E2E.Scratch -Recurse -Force
        }
        catch { Write-Log "could not remove the scratch folder: $($_.Exception.Message)" }
    }
}
