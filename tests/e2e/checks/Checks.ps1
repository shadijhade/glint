# The checks. Each one starts from the same state (Reset-App: the arena open, nothing selected, no dialogs) and uses
# real mouse and keyboard input; what it asserts is read back from the accessibility tree and from the disk.
# Dot-sourced by run.ps1 after the libraries. Add one with Add-Check <name> <what it proves> { ... }.

Set-StrictMode -Version 2.0

function Get-ArenaPath { param([string]$Name) return (Join-Path $script:E2E.Arena $Name) }

# A name written in Arabic, from code points (this file stays ASCII).
function New-ArabicName {
    param([string]$Suffix = '.txt')
    return ([string]([char]0x0628) + [char]0x064A + [char]0x062A + [char]0x0627 + $Suffix)   # beh, yeh, teh, alef
}

# Commits the open rename box by clicking away from it (the box commits on losing focus). Enter would commit too, but
# today it also reaches the list's Open (see the rename-enter check), which would launch a file's app.
function Complete-Rename {
    Focus-List
}

# ------------------------------------------------------------------------------------------------ launch

Add-Check 'launch' 'the app starts on the folder it was given, with a named window, tab, address bar and listing' {
    $main = Get-MainWindow
    Assert-True $main 'the main window exists and is called Glint'
    Assert-True (($main.Right - $main.Left) -gt 600) 'the window has a usable size'
    Assert-True (Find-El -Type Group -Name 'Address bar') "the address bar is named 'Address bar'"
    Assert-True (Find-El -Type ToolBar -Name 'Commands') "the command bar is named 'Commands'"
    Assert-True (Find-El -Type Group -Name 'Places') "the sidebar is named 'Places'"
    Assert-Equal (Get-TabTitle) $script:ArenaLeaf 'the tab is called after the folder'
    Assert-Equal (Get-RowNames) (Get-ArenaNames) 'the list shows the folder as the disk has it, folders first'
    $row = Get-Row 'alpha.txt'
    Assert-True ($row.Current.IsEnabled) 'rows are enabled'
}

# ------------------------------------------------------------------------------------------------ keyboard in the list

Add-Check 'keyboard-list' 'arrows, Home, End, type-ahead, Enter on a folder, Backspace and Alt+Up work in the list' {
    Select-Row 'alpha.txt'
    Send-Key 'down'
    Assert-Eventually { Test-SelectedIs 'beta.txt' } 'Down moves the selection to beta.txt'
    Send-Key 'down'
    Assert-Eventually { Test-SelectedIs 'dragme.txt' } 'Down again reaches dragme.txt'
    Send-Key 'up'
    Assert-Eventually { Test-SelectedIs 'beta.txt' } 'Up goes back to beta.txt'
    Send-Key 'end'
    $last = (Get-ArenaNames)[-1]
    Assert-Eventually { Test-SelectedIs $last } 'End selects the last row'
    Send-Key 'home'
    Assert-Eventually { Test-SelectedIs 'inbox' } 'Home selects the first row'
    Send-Key 'g'
    Assert-Eventually { Test-SelectedIs 'gamma.md' } 'typing g jumps to gamma.md'

    Send-Key 'home'
    Assert-Eventually { Test-SelectedIs 'inbox' } 'back on inbox'
    Send-Key 'enter'
    Assert-Eventually { (Get-TabTitle) -eq 'inbox' } 'Enter opens the folder (the tab is called inbox)'
    Assert-Eventually { Test-RowNamesAre 'inner.txt' } 'the folder lists its file'
    Send-Key 'backspace'
    Assert-Eventually { (Get-TabTitle) -eq $script:ArenaLeaf } 'Backspace goes back to the arena'
    Assert-Eventually { Test-RowsShown (Get-ArenaNames) } 'the arena lists everything again'

    Select-Row 'inbox'
    Send-Key 'enter'
    Assert-Eventually { (Get-TabTitle) -eq 'inbox' } 'inbox opens again'
    Send-Key 'alt+up'
    Assert-Eventually { (Get-TabTitle) -eq $script:ArenaLeaf } 'Alt+Up goes to the parent folder'
}

# ------------------------------------------------------------------------------------------------ address bar

Add-Check 'address-bar' 'a path typed into the address bar (Ctrl+L) opens that folder; Escape cancels; a crumb goes up' {
    $inbox = Get-ArenaPath 'inbox'
    Open-Path $inbox
    Assert-Eventually { (Get-TabTitle) -eq 'inbox' } 'typing the path of inbox and pressing Enter opens it'
    Assert-Eventually { Test-RowNamesAre 'inner.txt' } 'inbox shows its file'
    Assert-True (-not (Find-El -Type Edit -Name 'Address')) 'the address box closed after Enter'

    Send-Key 'ctrl+l'
    Assert-Eventually { Find-El -Type Edit -Name 'Address' } 'Ctrl+L opens the address box again'
    Send-Text 'C:\this-folder-does-not-exist-e2e'
    Send-Key 'escape'
    Assert-Eventually { -not (Find-El -Type Edit -Name 'Address') } 'Escape closes the address box'
    Assert-Equal (Get-TabTitle) 'inbox' 'Escape left the folder as it was'

    # A crumb of the breadcrumb bar goes up to that folder.
    $crumb = Find-El -Type Button -Name $script:ArenaLeaf
    Assert-True $crumb "the crumb 'arena' is on the bar"
    Invoke-Click $crumb
    Assert-Eventually { (Get-TabTitle) -eq $script:ArenaLeaf } 'clicking the crumb opens the arena'
    Assert-Eventually { Test-RowsShown (Get-ArenaNames) } 'and lists it'

    # The Up button.
    Select-Row 'inbox'
    Send-Key 'enter'
    Assert-Eventually { (Get-TabTitle) -eq 'inbox' } 'into inbox'
    $up = Find-El -Type Button -Name 'Up (Alt+Up)'
    Invoke-Click $up
    Assert-Eventually { (Get-TabTitle) -eq $script:ArenaLeaf } 'the Up button goes to the parent'
}

# ------------------------------------------------------------------------------------------------ menus, rename, undo

Add-Check 'context-menu' 'the folder menu opens on the empty list and New folder runs; Escape closes a menu' {
    $p = Get-EmptyListPoint
    Invoke-ClickAt $p.X $p.Y -Right
    Assert-Eventually { Get-OpenMenu -Name 'Folder menu' } "a right-click on the empty list opens the 'Folder menu'"
    $menu = Get-OpenMenu -Name 'Folder menu'
    $items = Get-Names (Find-Els -Type MenuItem -Root $menu)
    foreach ($want in 'New folder', 'Paste', 'Select all', 'Refresh') {
        Assert-True ($items -contains $want) "the folder menu has '$want' (it has: $($items -join ', '))"
    }
    Send-Key 'escape'
    Assert-Eventually { -not (Get-OpenMenu) } 'Escape closes the menu'

    Invoke-ClickAt $p.X $p.Y -Right
    Assert-Eventually { Get-OpenMenu -Name 'Folder menu' } 'the menu opens again'
    Select-MenuItem -Menu 'Folder menu' -Item 'New folder'
    Assert-Eventually { Get-RenameBox } "New folder puts the new folder's name in the rename box"
    Send-Key 'ctrl+a'
    Send-Text 'e2e-created'
    Complete-Rename
    Assert-Eventually { Test-Path -LiteralPath (Get-ArenaPath 'e2e-created') -PathType Container } 'the folder exists on disk under the typed name'
    Assert-Eventually { (Test-RowsShown @('e2e-created')) -and -not (Get-RenameBox) } 'the list shows it and the rename box is gone'
}

Add-Check 'rename-undo' 'Rename from the item menu opens the real rename box, renames on disk, and Ctrl+Z undoes it' {
    Select-Row 'beta.txt'
    Invoke-Click (Get-Row 'beta.txt') -Right
    Assert-Eventually { Get-OpenMenu -Name 'Item menu' } "a right-click on a row opens the 'Item menu'"
    Select-MenuItem -Menu 'Item menu' -Item 'Rename'
    Assert-Eventually { Get-RenameBox } 'the rename box opens'
    Send-Key 'ctrl+a'
    Send-Text 'renamed.txt'
    Complete-Rename
    Assert-Eventually { (Test-Path -LiteralPath (Get-ArenaPath 'renamed.txt')) -and -not (Test-Path -LiteralPath (Get-ArenaPath 'beta.txt')) } 'the file was renamed on disk'
    Assert-Eventually { (Test-RowsShown @('renamed.txt')) -and (Test-RowGone 'beta.txt') } 'the list shows the new name'

    # The keyboard way, with an Arabic name typed into the box (right to left input).
    $arabic = New-ArabicName
    Select-Row 'moveme.txt'
    Send-Key 'f2'
    Assert-Eventually { Get-RenameBox } 'F2 opens the rename box'
    Send-Key 'ctrl+a'
    Send-Text $arabic
    Complete-Rename
    Assert-Eventually { Test-Path -LiteralPath (Get-ArenaPath $arabic) } 'an Arabic name typed into the rename box reaches the disk intact'
    Assert-Eventually { Test-RowsShown @($arabic) } 'the list shows the Arabic name'

    # Undo the last rename (the Arabic one), then the one before it: two presses of Ctrl+Z.
    Select-Row 'alpha.txt'
    Send-Key 'ctrl+z'
    Assert-Eventually { (Test-Path -LiteralPath (Get-ArenaPath 'moveme.txt')) -and -not (Test-Path -LiteralPath (Get-ArenaPath $arabic)) } 'Ctrl+Z puts moveme.txt back'
    Send-Key 'ctrl+z'
    Assert-Eventually { (Test-Path -LiteralPath (Get-ArenaPath 'beta.txt')) -and -not (Test-Path -LiteralPath (Get-ArenaPath 'renamed.txt')) } 'a second Ctrl+Z puts beta.txt back'
    Assert-Eventually { (Test-RowsShown @('beta.txt', 'moveme.txt')) -and (Test-RowGone 'renamed.txt') } 'the list shows the original names again'
}

Add-Check 'rename-cancel-focus' 'after Escape closes the rename box the keyboard still works (Ctrl+L)' {
    Select-Row 'gamma.md'
    Send-Key 'f2'
    Assert-Eventually { Get-RenameBox } 'F2 opens the rename box'
    Send-Key 'escape'
    Assert-Eventually { -not (Get-RenameBox) } 'Escape closes it'
    Start-Sleep -Milliseconds 400
    Send-Key 'ctrl+l'
    Assert-Eventually { Find-El -Type Edit -Name 'Address' } 'Ctrl+L still opens the address box' 3
}

Add-Check 'rename-enter' 'Enter in the rename box commits the name and does nothing else' {
    Select-Row 'target2'
    Send-Key 'f2'
    Assert-Eventually { Get-RenameBox } 'F2 opens the rename box on a folder (a folder, so a stray Open is harmless)'
    Send-Key 'ctrl+a'
    Send-Text 'e2e-entered'
    Send-Key 'enter'
    Assert-Eventually { Test-Path -LiteralPath (Get-ArenaPath 'e2e-entered') -PathType Container } 'Enter renamed the folder on disk'
    Start-Sleep -Milliseconds 1500
    Assert-Equal (Get-TabTitle) $script:ArenaLeaf 'the tab is still on the arena (Enter must not also open the item)'
}

# ------------------------------------------------------------------------------------------------ the aware menu

Add-Check 'smart-menu' "an archive's menu starts with its head and puts Extract here first; typing finds a row and Enter runs it" {
    # The arena has no archive: one is made here, and the list shows it by itself (the folder is watched). The next
    # check's reset takes it away again.
    $zip = Get-ArenaPath 'pack.zip'
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::CreateFromDirectory((Get-ArenaPath 'inbox'), $zip)
    Assert-Eventually { Test-RowsShown @('pack.zip') } 'the list shows the new archive' 10

    Select-Row 'pack.zip'
    Invoke-Click (Get-Row 'pack.zip') -Right
    Assert-Eventually { Get-OpenMenu -Name 'Item menu' } "a right-click on the archive opens the 'Item menu'"
    $menu = Get-OpenMenu -Name 'Item menu'
    Assert-True (Find-El -Type Group -Name 'pack.zip' -Root $menu) 'the menu starts with a head named after the item'
    $items = @(Get-Names (Find-Els -Type MenuItem -Root $menu))
    $extract = [array]::IndexOf($items, 'Extract here')
    $open = [array]::IndexOf($items, 'Open')
    Assert-True ($extract -ge 0 -and $open -ge 0 -and $extract -lt $open) "Extract here is put forward, above Open (items: $($items -join ', '))"

    # Typing in an open menu finds a row anywhere in it; Enter runs the first one found.
    Send-Text 'ren'
    Assert-Eventually { (@(Get-Names (Find-Els -Type MenuItem -Root (Get-OpenMenu -Name 'Item menu'))))[0] -eq 'Rename' } 'typing "ren" leaves the rows that match, Rename first'
    Send-Key 'enter'
    Assert-Eventually { Get-RenameBox } 'Enter runs the row found: the rename box opens on the archive'
    Send-Key 'escape'
    Assert-Eventually { -not (Get-RenameBox) } 'Escape closes the box'
    Assert-True (Test-Path -LiteralPath $zip) 'and the archive keeps its name'
    Focus-List
}

# ------------------------------------------------------------------------------------------------ dragging files

Add-Check 'drag-copy' 'a real mouse drag of a file onto a folder row copies it (the original stays)' {
    $src = Get-ArenaPath 'dragme.txt'
    $before = Get-Content -LiteralPath $src -Raw
    Invoke-Drag (Get-Row 'dragme.txt') (Get-Row 'target')
    Assert-Eventually { Test-Path -LiteralPath (Get-ArenaPath 'target\dragme.txt') } 'the file arrived in target' 8
    Assert-True (Test-Path -LiteralPath $src) 'the original is still where it was (a drag copies by default)'
    Assert-Equal (Get-Content -LiteralPath (Get-ArenaPath 'target\dragme.txt') -Raw) $before 'the copy has the same contents'
    Assert-True (Test-RowsShown @('dragme.txt')) 'the list still shows the original'
}

Add-Check 'drag-move-shift' 'the same drag with Shift held moves the file' {
    $src = Get-ArenaPath 'moveme.txt'
    $before = Get-Content -LiteralPath $src -Raw
    Invoke-Drag (Get-Row 'moveme.txt') (Get-Row 'target2') -Shift
    Assert-Eventually { Test-Path -LiteralPath (Get-ArenaPath 'target2\moveme.txt') } 'the file arrived in target2' 8
    Assert-Eventually { -not (Test-Path -LiteralPath $src) } 'and left its folder (Shift moves)'
    Assert-Equal (Get-Content -LiteralPath (Get-ArenaPath 'target2\moveme.txt') -Raw) $before 'with its contents'
    Assert-Eventually { Test-RowGone 'moveme.txt' } 'the list no longer shows it'
}

# Drags the file out of a real File Explorer window onto Glint's 'target' folder row. `$Prep` is what happens in Glint
# just before the Explorer window opens: 'mouse' moves the pointer over Glint's list, as a person reaching for the
# window does; 'key' presses a key in Glint instead. Returns $true when the file arrived in the folder, $false when
# it did not, $null when the run cannot do the drag on this PC (the check is then marked skipped).
function Invoke-ExplorerDrop {
    param([ValidateSet('mouse', 'key')][string]$Prep)
    $source = Join-Path $script:E2E.Scratch 'from-explorer'
    Assert-Eventually { Get-Row 'target' } "Glint lists the 'target' folder"
    if ($Prep -eq 'key') { Send-Key 'escape'; Start-Sleep -Milliseconds 300 }
    else {
        $p = Get-EmptyListPoint
        [GlintE2E.Win]::MoveTo($p.X, $p.Y); Start-Sleep -Milliseconds 200
        [GlintE2E.Win]::MoveTo($p.X + 30, $p.Y); Start-Sleep -Milliseconds 300
    }
    $win = Open-ExplorerWindow $source
    if (-not $win) {
        Set-Skip 'Windows opened the folder in a tab of an Explorer window that was already open; the run does not touch windows it did not open'
        return $null
    }
    $glint = Get-MainWindow
    # Explorer to the lower right; Glint's list (its folder rows) stays visible on the left.
    $screen = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
    [GlintE2E.Win]::Place($win.Handle, $glint.Left + 520, $glint.Top + 180, [Math]::Min(640, $screen.Right - $glint.Left - 520), 440)
    Start-Sleep -Milliseconds 1000
    [void][GlintE2E.Win]::Focus($win.Handle)
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($win.Handle)
    Assert-Eventually { @(Find-Els -Type ListItem -Name 'fromexplorer*' -Like -Root $root).Count -gt 0 } 'File Explorer lists the file' 10
    $item = @(Find-Els -Type ListItem -Name 'fromexplorer*' -Like -Root $root)[0]
    Assert-Eventually { Get-Row 'target' } "Glint still lists the 'target' folder"
    $a = Get-Point $item -FromStart 55   # the name, not the empty middle of the row: that would start a marquee
    $b = Get-Point (Get-Row 'target') -FromStart 70
    Assert-True ([GlintE2E.Win]::TopWindowAt($b.X, $b.Y) -eq $glint.Handle) 'the drop point is on Glint, not covered by the Explorer window'
    Write-Log ("  dragging from File Explorer at $($a.X),$($a.Y) to Glint's 'target' row at $($b.X),$($b.Y)")
    Invoke-DragPoints -X1 $a.X -Y1 $a.Y -X2 $b.X -Y2 $b.Y -HoldMs 700
    $arrived = Wait-Until -Seconds 6 -Cond { Test-Path -LiteralPath (Get-ArenaPath 'target\fromexplorer.txt') }
    Close-OurExplorer
    return $arrived
}

Add-Check 'drop-from-explorer' 'a file dragged out of a real File Explorer window onto a folder row is copied in' {
    $arrived = Invoke-ExplorerDrop -Prep mouse
    if ($null -eq $arrived) { return }
    Assert-True $arrived 'the file dropped from File Explorer arrived in the target folder'
    Assert-Equal (Get-Content -LiteralPath (Get-ArenaPath 'target\fromexplorer.txt') -Raw).Trim() 'dropped from File Explorer' 'with its contents'
    Assert-True (Test-Path -LiteralPath (Join-Path $script:E2E.Scratch 'from-explorer\fromexplorer.txt')) 'and it is still in the Explorer folder (a drop copies by default)'
    Assert-Eventually { (Get-OpenMenu) -eq $null } 'no menu or dialog opened by the drop'
}

Add-Check 'drop-after-key' 'a file dragged in from File Explorer is also accepted after a key was pressed in Glint' {
    $arrived = Invoke-ExplorerDrop -Prep key
    if ($null -eq $arrived) { return }
    Assert-True $arrived 'the file dropped from File Explorer arrived in the target folder'
}

# ------------------------------------------------------------------------------------------------ Quick Look

Add-Check 'quick-look' 'Space opens Quick Look on the selected file, an arrow moves to the next item, Space closes it' {
    Select-Row 'alpha.txt'
    Send-Key 'space'
    Assert-Eventually { Get-QuickLook } 'Space opens a Quick Look window'
    Assert-Equal (Get-QuickLook).Current.Name 'Quick Look: alpha.txt' 'it is named after the file'
    Assert-Eventually { (Get-ElText (Find-El -Type Edit -Name 'alpha.txt' -Root (Get-QuickLook))) -like '*the needle is in this file*' } 'it shows the text of the file'
    Send-Key 'right'
    Assert-Eventually { (Get-QuickLook) -and (Get-QuickLook).Current.Name -eq 'Quick Look: beta.txt' } 'the right arrow shows the next item in the folder'
    Send-Key 'space'
    Assert-Eventually { -not (Get-QuickLook) } 'Space closes it'
    Assert-Eventually { Test-SelectedIs 'beta.txt' } 'the selection followed to beta.txt'
    Send-Key 'space'
    Assert-Eventually { Get-QuickLook } 'it opens again'
    Send-Key 'escape'
    Assert-Eventually { -not (Get-QuickLook) } 'Escape closes it'
}

# ------------------------------------------------------------------------------------------------ command bar


Add-Check 'command-bar' 'the command bar buttons work: New folder, Copy and Paste, Rename, Grid and List, palette, View menu' {
    # New folder
    Click-Button 'New folder'
    Assert-Eventually { Get-RenameBox } 'the New folder button opens the rename box on a new folder'
    Send-Key 'ctrl+a'
    Send-Text 'bar-folder'
    Complete-Rename
    Assert-Eventually { Test-Path -LiteralPath (Get-ArenaPath 'bar-folder') -PathType Container } 'the folder exists on disk'

    # Copy and Paste: Windows names the pasted copy "alpha - Copy.txt".
    Select-Row 'alpha.txt'
    Assert-Eventually { (Get-Button 'Copy (Ctrl+C)').Current.IsEnabled } 'Copy is enabled with a file selected'
    Click-Button 'Copy (Ctrl+C)'
    Assert-Eventually { @([GlintE2E.ClipboardBackup]::FilesNow() | Where-Object { $_ -like '*\alpha.txt' }).Count -eq 1 } 'Copy put the file on the real clipboard'
    Click-Button 'Paste (Ctrl+V)'
    Assert-Eventually { Test-Path -LiteralPath (Get-ArenaPath 'alpha - Copy.txt') } 'Paste made the copy beside the original' 10
    Assert-Eventually { Test-RowsShown @('alpha - Copy.txt') } 'the list shows the copy'

    # Rename opens the box; Escape leaves the name as it was.
    Select-Row 'gamma.md'
    Click-Button 'Rename (F2)*'
    Assert-Eventually { Get-RenameBox } 'the Rename button opens the rename box on the selected file'
    Send-Key 'escape'
    Assert-Eventually { -not (Get-RenameBox) } 'Escape closes the box'
    Assert-True (Test-Path -LiteralPath (Get-ArenaPath 'gamma.md')) 'and the file keeps its name'
    Focus-List   # nothing holds the keyboard now (see rename-cancel-focus), so a person would click into the list

    # Grid and List: the same items, laid out in tiles and then in rows again.
    Click-Button 'Grid, with thumbnails*'
    Assert-Eventually { Test-GridLayout } 'the Grid button lays the items out in tiles'
    Assert-True (Test-RowsShown @('alpha.txt', 'target')) 'the same items are there'
    Click-Button 'List (Ctrl+Shift+6)'
    Assert-Eventually { -not (Test-GridLayout) } 'the List button puts them in rows again'

    # The command palette: opened by its button, runs a command by name.
    Click-Button 'Command palette (Ctrl+K)'
    Assert-Eventually { Find-El -Type Group -Name 'Command palette' } 'the Command palette button opens the palette'
    Send-Text 'grid'
    Assert-Eventually { Find-El -Type ListItem -Name 'Show as a grid*' -Like } 'typing "grid" finds the grid command'
    Send-Key 'enter'
    Assert-Eventually { (-not (Find-El -Type Group -Name 'Command palette')) -and (Test-GridLayout) } 'Enter runs it: the palette closes and the files are in a grid'
    Click-Button 'List (Ctrl+Shift+6)'
    Assert-Eventually { -not (Test-GridLayout) } 'back to the list'

    # The View menu. A bar with room shows its four switches as buttons instead (a 1100 px window at 100% scale has
    # the room; at 125% it folds them into the menu): either way all four are there.
    Clear-Toasts
    if (Get-Button 'View: hidden items*') {
        Click-Button 'View: hidden items*'
        Assert-Eventually { Get-OpenMenu -Name 'View' } 'the View button opens its menu'
        $names = Get-Names (Find-Els -Type CheckBox -Root (Get-OpenMenu -Name 'View'))
        foreach ($want in 'Show hidden items', 'Preview pane', 'Dual pane', 'Compare panes') {
            Assert-True ($names -contains $want) "the View menu has '$want'"
        }
        Send-Key 'escape'
        Assert-Eventually { -not (Get-OpenMenu) } 'Escape closes it'
    }
    else {
        foreach ($want in 'Show hidden items*', 'Preview pane*', 'Dual pane*', 'Compare panes*') {
            Assert-True (Get-Button $want) "the bar has the '$want' switch"
        }
    }

    # The Tools menu opens too (its items are tools that may be missing on this PC, so only that it opens is checked).
    Click-Button 'Tools'
    Assert-Eventually { Get-OpenMenu } 'the Tools button opens a menu'
    Send-Key 'escape'
    Assert-Eventually { -not (Get-OpenMenu) } 'Escape closes it'
}

# ------------------------------------------------------------------------------------------------ the zoom

# Turns the wheel with Ctrl held, the pointer on `$El`: a notch at a time (above zero: away from you, the zoom in)
# until `$Until` holds, four at most. One notch is a step of zoom where Windows scrolls three lines a notch, which
# is its default; a PC set to fewer needs more of them.
function Invoke-CtrlWheel {
    param($El, [int]$Direction, [scriptblock]$Until, [int]$FromStart = -1, $Window)
    Set-Foreground $Window
    $pt = Get-Point $El -FromStart $FromStart
    [GlintE2E.Win]::KeyDown([uint16]0x11)
    try {
        for ($i = 0; $i -lt 4; $i++) {
            [GlintE2E.Win]::Wheel($pt.X, $pt.Y, $Direction)
            if (Wait-Until -Seconds 0.25 -Cond $Until) { break }
        }
    }
    finally { [GlintE2E.Win]::KeyUp([uint16]0x11) }
}

Add-Check 'zoom-layout' 'Ctrl and the wheel over a folder change the layout (list, tiles, list) and never open the folder or leave it' {
    # In, over a folder's row: 0.13.0 opened the folder.
    Invoke-CtrlWheel (Get-Row 'inbox') 1 { Test-GridLayout } -FromStart 70
    Assert-Eventually { Test-GridLayout } "a notch in over a folder's row lays the folder out in tiles"
    Assert-Equal (Get-TabTitle) $script:ArenaLeaf 'the folder is the one it was'
    Assert-Equal (Get-RowNames) (Get-ArenaNames) 'with everything it holds, in the same order'
    Start-Sleep -Milliseconds 600   # the wheel's spin ends, and the tiles have landed

    # Out, over the folder's tile at the smallest size: 0.13.0 went to the folder above.
    Invoke-CtrlWheel (Get-Row 'inbox') -1 { -not (Test-GridLayout) }
    Assert-Eventually { -not (Test-GridLayout) } 'a notch out from the smallest tiles is the list again'
    Assert-Equal (Get-TabTitle) $script:ArenaLeaf 'still the same folder'
    Assert-Equal (Get-RowNames) (Get-ArenaNames) 'and the same items'
    Start-Sleep -Milliseconds 600

    # Out again: the list is as far out as the zoom goes.
    Invoke-CtrlWheel (Get-Row 'inbox') -1 { $false } -FromStart 70
    Start-Sleep -Milliseconds 400
    Assert-Equal (Get-TabTitle) $script:ArenaLeaf 'out from the list goes nowhere'
    Assert-True (-not (Test-GridLayout)) 'and the list stays a list'
}

# ------------------------------------------------------------------------------------------------ dual pane, compare

Add-Check 'dual-pane-compare' 'Ctrl+Shift+D shows two panes, F6 switches, Ctrl+Shift+M marks what differs, both toggle off' {
    Send-Key 'ctrl+shift+d'
    Assert-Eventually { @(Get-FileLists).Count -eq 2 } 'Ctrl+Shift+D shows two file lists'
    Assert-True (Find-El -Type Tab -Name 'Left pane tabs') "the left pane's tabs are named"
    Assert-True (Find-El -Type Tab -Name 'Right pane tabs') "the right pane's tabs are named"
    Assert-Equal (Get-RowNames -List 1) (Get-ArenaNames) 'the second pane starts in the same folder'

    # Take the right pane into inbox: click its row, Enter.
    Select-Row 'inbox' -List 1
    Send-Key 'enter'
    Assert-Eventually { Test-RowNamesAre 'inner.txt' -List 1 } 'the right pane shows inbox while the left one stays'
    Assert-True (Test-RowsShown (Get-ArenaNames) -List 0) 'the left pane still shows the arena'

    Send-Key 'ctrl+shift+m'
    Assert-Eventually { Find-El -Type Group -Name 'Only here: alpha.txt' } 'compare panes marks alpha.txt as only in the left pane' 8
    Assert-True (Find-El -Type Group -Name 'Only here: inner.txt') 'and inner.txt as only in the right one'
    Send-Key 'ctrl+shift+m'
    Assert-Eventually { -not (Find-El -Type Group -Name 'Only here: alpha.txt') } 'a second Ctrl+Shift+M takes the marks off'

    Send-Key 'f6'
    Start-Sleep -Milliseconds 300
    Send-Key 'ctrl+shift+d'
    Assert-Eventually { @(Get-FileLists).Count -eq 1 } 'Ctrl+Shift+D closes the second pane'
}

# ------------------------------------------------------------------------------------------------ the one search

Add-Check 'search-inside' 'Ctrl+Shift+F goes to the search box; a word and Enter list the file that holds it, with the line under it; Escape goes back' {
    Send-Key 'ctrl+shift+f'
    Assert-Eventually { Find-El -Type Edit -Name 'Search * and its subfolders.' -Like } 'Ctrl+Shift+F puts the keyboard in the search box'
    Send-Text 'needle'
    Send-Key 'enter'
    Assert-Eventually { Get-SearchBar } 'Enter shows the search results'
    Assert-Eventually { Find-El -Type Group -Name 'Inside files' } 'the results have a part for what was found inside files' 15
    Assert-Eventually { Find-El -Type Button -Name 'Line 2: the needle is in this file' } 'the matching line is listed, with its number' 15
    Assert-True (Get-Row -Name 'alpha.txt') 'under the file it is in, a row like any other'
    Send-Key 'escape'
    Assert-Eventually { -not (Get-SearchBar) } 'Escape in the box leaves the results'
    Assert-Eventually { Test-RowsShown (Get-ArenaNames) } 'and the folder is listed'
}

Add-Check 'search-history' 'a folder opened from search results: Backspace brings the results back with it selected, the mouse''s back button the folder searched, its forward button the results again' {
    Send-Key 'ctrl+f'
    Assert-Eventually { Find-El -Type Edit -Name 'Search * and its subfolders.' -Like } 'Ctrl+F puts the keyboard in the search box'
    # 'inbo', not 'inbox': a tab showing results is called after their words, so the two titles differ.
    Send-Text 'inbo'
    Send-Key 'enter'
    Assert-Eventually { Get-SearchBar } 'Enter shows the search results'
    Assert-Eventually { Get-Row -Name 'inbox' } 'the folder inbox is among them'
    Select-Row 'inbox'
    Send-Key 'enter'
    Assert-Eventually { (Get-TabTitle) -eq 'inbox' -and -not (Get-SearchBar) } 'Enter opens the folder from the results'
    Assert-Eventually { Test-RowNamesAre 'inner.txt' } 'which lists its file'
    Send-Key 'backspace'
    Assert-Eventually { Get-SearchBar } 'Backspace brings the results back'
    Assert-Eventually { (Get-TabTitle) -eq 'inbo' } 'with their words'
    Assert-Eventually { Find-El -Type Edit -Name ('Search ' + $script:ArenaLeaf + ' and its subfolders.') } 'in the folder they were found in'
    Assert-Eventually { Test-SelectedIs 'inbox' } 'with the folder opened from them selected'
    $p = Get-EmptyListPoint
    Invoke-SideButton $p.X $p.Y
    Assert-Eventually { -not (Get-SearchBar) } 'the mouse''s back button leaves the results'
    Assert-Eventually { Test-RowsShown (Get-ArenaNames) } 'for the folder searched, listed whole'
    Invoke-SideButton $p.X $p.Y -Forward
    Assert-Eventually { Get-SearchBar } 'its forward button brings the results back once more'
    Assert-Eventually { Test-SelectedIs 'inbox' } 'inbox still selected'
    Send-Key 'alt+right'
    Assert-Eventually { (Get-TabTitle) -eq 'inbox' -and -not (Get-SearchBar) } 'Alt+Right goes on to the folder opened from them'
    Assert-Eventually { Test-RowNamesAre 'inner.txt' } 'which lists its file again'
}

# ------------------------------------------------------------------------------------------------ settings, tabs, sidebar

Add-Check 'settings' 'Ctrl+, opens Settings with its sections; Escape closes it' {
    Send-Key 'ctrl+,'
    Assert-Eventually { Get-SettingsDialog } 'Ctrl+, opens Settings'
    Assert-True (Find-El -Type Group -Name 'Settings sections') 'it lists its sections'
    $section = Find-El -Type Button -Name 'Appearance'
    Assert-True $section "with an 'Appearance' section"
    Invoke-Click $section
    Assert-Eventually { Find-El -Type Group -Name 'Appearance' } 'choosing a section shows its page'
    Send-Key 'escape'
    Assert-Eventually { -not (Get-SettingsDialog) } 'Escape closes Settings'
    Assert-True (Get-FileList) 'and the files are back under it'
}

Add-Check 'tabs-sidebar' 'Ctrl+T opens a tab, a sidebar place opens in it, Ctrl+W closes it' {
    Send-Key 'ctrl+t'
    Assert-Eventually { @(Find-Els -Type TabItem).Count -eq 2 } 'Ctrl+T adds a tab'
    $docs = Find-El -Type Button -Name 'Documents'
    Assert-True $docs "the sidebar has 'Documents'"
    Invoke-Click $docs
    Assert-Eventually { (Get-TabTitle) -eq 'Documents' } 'clicking a sidebar place opens it in the tab'
    Send-Key 'ctrl+w'
    Assert-Eventually { @(Find-Els -Type TabItem).Count -eq 1 } 'Ctrl+W closes the tab'
    Assert-Eventually { (Get-TabTitle) -eq $script:ArenaLeaf } 'the first tab, still on the arena, is back'
}

# ------------------------------------------------------------------------------------------------ quitting

Add-Check 'quit' 'closing the window quits the app cleanly (exit code 0) and it saves its state' -EndsApp {
    $p = $script:E2E.Process
    $main = Get-MainWindow
    [GlintE2E.Win]::Close($main.Handle)
    if (-not $p.WaitForExit(10000)) {
        # Still running: say whether the window was only hidden (Glint kept running in the tray), is still shown
        # (a question asked before quitting), or closed with the process left behind (quitting hangs), and which
        # windows the process still has. Without this a failure here said nothing about why.
        $after = if (-not [GlintE2E.Win]::Exists($main.Handle)) { 'the window closed but the process stayed' }
                 elseif ([GlintE2E.Win]::Describe($main.Handle).Visible) { 'the window is still shown' }
                 else { 'the window was hidden, as when Glint keeps running in the tray' }
        $windows = @([GlintE2E.Win]::TopWindows($p.Id) | Where-Object { $_.Class -ne 'IME' -and $_.Class -ne 'MSCTFIME UI' } |
            ForEach-Object { '{0} "{1}"{2}' -f $_.Class, $_.Title, $(if ($_.Visible) { '' } else { ' (hidden)' }) }) -join ', '
        throw "the process didn't exit within 10 s of closing the window: $after; its windows: $windows"
    }
    Assert-Equal $p.ExitCode 0 'with exit code 0'
    $state = Get-ProfileFile Roaming 'state.json'
    Assert-True (Test-Path -LiteralPath $state) 'state.json was written on the way out'
    $json = Get-Content -LiteralPath $state -Raw -Encoding UTF8 | ConvertFrom-Json
    Assert-True ($null -ne $json.state.session) 'it holds the session'
}
