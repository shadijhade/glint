# Helpers that know Glint: its accessible names, how to get from any state back to the arena, menus, dialogs, and
# the File Explorer window used for the drop check. Dot-sourced after Harness.ps1.

Set-StrictMode -Version 2.0

$script:ArenaLeaf = 'arena'

function Get-Names {
    param($Els)
    return @($Els | ForEach-Object { $_.Current.Name })
}

# ---- reading the screen ----

function Test-RowsShown {
    param([string[]]$Names, [int]$List = 0)
    $have = Get-RowNames -List $List
    foreach ($n in $Names) { if ($have -notcontains $n) { return $false } }
    return $true
}

function Test-RowGone {
    param([string]$Name, [int]$List = 0)
    return ((Get-RowNames -List $List) -notcontains $Name)
}

# The selected rows are exactly `$Names` (compared in order).
function Test-SelectedIs {
    param([string[]]$Names, [int]$List = 0)
    return (((Get-SelectedNames -List $List) -join '|') -eq ($Names -join '|'))
}

# The list shows exactly `$Names`, in that order.
function Test-RowNamesAre {
    param([string[]]$Names, [int]$List = 0)
    return (((Get-RowNames -List $List) -join '|') -eq ($Names -join '|'))
}

# True when the items are laid out in tiles: rows of a list share their left edge, tiles don't.
function Test-GridLayout {
    $rows = @(Get-Rows)
    if ($rows.Count -lt 3) { return $false }
    $lefts = @($rows | Select-Object -First 4 | ForEach-Object { (Get-Rect $_.El).L } | Sort-Object -Unique)
    return ($lefts.Count -gt 1)
}

function Get-RenameBox { return (Find-El -Type Edit -Name 'New name') }

# The open menu (any menu, or the one called `$Name`), or $null.
function Get-OpenMenu {
    param([string]$Name)
    if ($Name) { return (Find-El -Type Menu -Name $Name) }
    return (Find-El -Type Menu)
}

function Get-QuickLook { return (Find-El -Type Window -Name 'Quick Look*' -Like) }

function Get-SettingsDialog { return (Find-El -Type Group -Name 'Settings') }

function Get-TextSearch { return (Find-El -Type Group -Name 'Text search') }

# Clicks the item called `$Item` of the open menu `$Menu`, found inside it (the toolbar has a button of the same name).
function Select-MenuItem {
    param([string]$Menu, [string]$Item, [string]$Type = 'MenuItem')
    $m = $null
    if (-not (Wait-Until -Seconds 4 -Cond { $script:m2 = Get-OpenMenu -Name $Menu; $null -ne $script:m2 })) { throw "the menu '$Menu' did not open" }
    $m = $script:m2
    $el = Find-El -Type $Type -Name $Item -Root $m
    if (-not $el) {
        $have = (Find-Els -Root $m | ForEach-Object { $_.Current.Name }) -join ' | '
        throw "no item '$Item' in the menu '$Menu' (it has: $have)"
    }
    Invoke-Click $el
}

# ---- doing things ----

# Clicks a row until it is the selection (the first click after a state change can land before the list is ready).
function Select-Row {
    param([string]$Name, [int]$List = 0, [switch]$Add)
    for ($try = 0; $try -lt 3; $try++) {
        $row = Get-Row -Name $Name -List $List
        if (-not $row) { Start-Sleep -Milliseconds 300; continue }
        Invoke-Click $row -FromStart 70
        if (Wait-Until -Seconds 2 -Cond { (Get-SelectedNames -List $List) -contains $Name }) { return }
    }
    throw "could not select the row '$Name' (rows: $((Get-RowNames -List $List) -join ', '))"
}

# Types a path into the address bar (Ctrl+L) and presses Enter.
function Open-Path {
    param([string]$Path)
    Send-Key 'ctrl+l'
    if (-not (Wait-Until -Seconds 3 -Cond { $null -ne (Find-El -Type Edit -Name 'Address') })) { throw 'Ctrl+L did not open the address box' }
    Start-Sleep -Milliseconds 150
    Send-Key 'ctrl+a'
    Send-Text $Path
    Start-Sleep -Milliseconds 250
    Send-Key 'enter'
}

function Get-ListGeometry {
    $l = Get-FileList
    if (-not $l) { throw 'no file list on screen' }
    return (Get-Rect $l)
}

# A point in the empty part of the list, below the last row (a right-click there opens the folder's menu).
function Get-EmptyListPoint {
    $r = Get-ListGeometry
    return @{ X = $r.L + 80; Y = $r.B - 30 }
}

# Clicks in the empty part of the list: gives the list the keyboard (a closed rename box leaves nothing focused: see the
# rename-cancel-focus check) and clears the selection.
function Focus-List {
    $p = Get-EmptyListPoint
    Invoke-ClickAt $p.X $p.Y
    Start-Sleep -Milliseconds 200
}

# Gets the app from whatever a check left it in to the same start: no menu or dialog, Files mode, one pane, one tab,
# the arena open with its original contents, nothing selected.
function Reset-App {
    $main = Get-MainWindow
    if (-not $main) { throw 'the Glint window is gone' }
    Set-Foreground $main

    for ($i = 0; $i -lt 4; $i++) {
        $overlay = (Get-OpenMenu) -or (Get-QuickLook) -or (Get-SettingsDialog) -or (Find-El -Type Edit -Name 'Address') -or (Find-El -Type Group -Name 'Command palette') -or (Get-RenameBox)
        if (-not $overlay) { break }
        Send-Key 'escape'
        Start-Sleep -Milliseconds 250
    }
    if (Get-TextSearch) {
        $files = Find-El -Type Button -Name 'Files mode'
        if ($files) { Invoke-Click $files }
        [void](Wait-Until -Seconds 3 -Cond { -not (Get-TextSearch) })
    }
    # After a rename or a navigation nothing may hold the keyboard (the box that had it is gone): a click in the empty
    # part of the list gives it to the list, so the keys below reach the app. It also clears the selection.
    if (Get-FileList) {
        $p = Get-EmptyListPoint
        Invoke-ClickAt $p.X $p.Y
        Start-Sleep -Milliseconds 200
    }
    if (@(Get-FileLists).Count -gt 1) {
        Send-Key 'ctrl+shift+d'
        [void](Wait-Until -Seconds 3 -Cond { @(Get-FileLists).Count -le 1 })
    }
    $tabs = @(Find-Els -Type TabItem)
    $guard = 0
    while ($tabs.Count -gt 1 -and $guard -lt 6) {
        Send-Key 'ctrl+w'
        Start-Sleep -Milliseconds 300
        $tabs = @(Find-Els -Type TabItem)
        $guard++
    }
    if ((Get-TabTitle) -ne $script:ArenaLeaf) {
        Open-Path $script:E2E.Arena
        if (-not (Wait-Until -Seconds 6 -Cond { (Get-TabTitle) -eq $script:ArenaLeaf })) { throw 'could not get back to the arena folder' }
    }
    Reset-Arena
    Send-Key 'f5'
    $expected = Get-ArenaNames
    if (-not (Wait-Until -Seconds 8 -Cond { @(Get-RowNames).Count -eq $expected.Count -and (Test-RowsShown $expected) })) {
        throw ("the arena's listing is not as expected: " + ((Get-RowNames) -join ', '))
    }
    # Grid back to list, if a check left it so.
    $files = Get-FileList
    if ($files) {
        $rows = @(Get-Rows)
        if ($rows.Count -ge 2 -and (Get-Rect $rows[0].El).L -ne (Get-Rect $rows[1].El).L) {
            $list = Find-El -Type Button -Name 'List (Ctrl+Shift+6)'
            if ($list) { Invoke-Click $list; Start-Sleep -Milliseconds 400 }
        }
    }
    # Nothing selected: a click in the empty part of the list clears the selection.
    if (@(Get-SelectedNames).Count -gt 0) {
        $p = Get-EmptyListPoint
        Invoke-ClickAt $p.X $p.Y
        Start-Sleep -Milliseconds 200
    }
}

# ---- File Explorer ----

# Opens a File Explorer window on `$Folder` and returns its handle, or $null when Windows put the folder in a tab of
# a window that was already open (the run never touches windows it did not open).
function Open-ExplorerWindow {
    param([string]$Folder)
    $before = @([GlintE2E.Win]::WindowsOfClass('CabinetWClass') | ForEach-Object { $_.Handle })
    Start-Process -FilePath 'explorer.exe' -ArgumentList ('"' + $Folder + '"') | Out-Null
    $leaf = Split-Path $Folder -Leaf
    $found = $null
    $ok = Wait-Until -Seconds 12 -Cond {
        foreach ($w in [GlintE2E.Win]::WindowsOfClass('CabinetWClass')) {
            # Explorer shows the folder's name, or its whole path when that setting is on: both end in the leaf name.
            if ($w.Visible -and ($before -notcontains $w.Handle) -and $w.Title -like "*$leaf*") { $script:found2 = $w; return $true }
        }
        return $false
    }
    if (-not $ok) { return $null }
    $script:E2E.Explorer = $script:found2.Handle
    return $script:found2
}

# ---- buttons and notifications ----

function Get-Button {
    param([string]$Name)
    if ($Name.Contains('*')) { return (Find-El -Type Button -Name $Name -Like) }
    return (Find-El -Type Button -Name $Name)
}

# A notification ("Copied 1 item") floats over the top of the toolbar for a few seconds and takes the clicks meant for
# the buttons under it. Its own close button is a small square near the top: click it away.
function Clear-Toasts {
    foreach ($b in @(Find-Els -Type Button -Name 'Close')) {
        $r = Get-Rect $b
        if ($r.W -le 24 -and $r.T -lt 140) {
            Invoke-Click $b
            Start-Sleep -Milliseconds 250
        }
    }
}

# Clicks a button of the window by its accessible name (a `*` in the name matches anything), after clearing any toast.
function Click-Button {
    param([string]$Name)
    Clear-Toasts
    $b = Get-Button $Name
    if (-not $b) { throw "no button '$Name' on screen" }
    Invoke-Click $b
}
