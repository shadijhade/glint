# The MSIX package's logos (scripts/msix.mjs), drawn from Glint's icon at each scale Windows asks for:
# the app's icon (Start, the taskbar, the Store's lists) at every target size, plain and unplated, and
# the tiles with room around the icon. Usage: msix-assets.ps1 -Source <icon.png> -Out <Assets folder>
param([Parameter(Mandatory)][string]$Source, [Parameter(Mandatory)][string]$Out)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$icon = [System.Drawing.Image]::FromFile($Source)

# A $width x $height picture with the icon in its middle, $fill of the shorter side.
function Write-Logo([int]$width, [int]$height, [double]$fill, [string]$name) {
    $bitmap = New-Object System.Drawing.Bitmap $width, $height, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $graphics.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $graphics.Clear([System.Drawing.Color]::Transparent)
    $size = [int][Math]::Round([Math]::Min($width, $height) * $fill)
    $place = New-Object System.Drawing.Rectangle ([int](($width - $size) / 2)), ([int](($height - $size) / 2)), $size, $size
    # Edges sampled as if the icon went on, so its border doesn't fade.
    $attributes = New-Object System.Drawing.Imaging.ImageAttributes
    $attributes.SetWrapMode([System.Drawing.Drawing2D.WrapMode]::TileFlipXY)
    $graphics.DrawImage($icon, $place, 0, 0, $icon.Width, $icon.Height, [System.Drawing.GraphicsUnit]::Pixel, $attributes)
    $graphics.Dispose()
    $bitmap.Save((Join-Path $Out $name), [System.Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
}

$scales = 100, 125, 150, 200, 400
foreach ($scale in $scales) {
    $at = $scale / 100
    Write-Logo ([Math]::Round(44 * $at)) ([Math]::Round(44 * $at)) 1 "Square44x44Logo.scale-$scale.png"
    Write-Logo ([Math]::Round(50 * $at)) ([Math]::Round(50 * $at)) 1 "StoreLogo.scale-$scale.png"
    Write-Logo ([Math]::Round(71 * $at)) ([Math]::Round(71 * $at)) 0.66 "Square71x71Logo.scale-$scale.png"
    Write-Logo ([Math]::Round(150 * $at)) ([Math]::Round(150 * $at)) 0.6 "Square150x150Logo.scale-$scale.png"
    Write-Logo ([Math]::Round(310 * $at)) ([Math]::Round(150 * $at)) 0.6 "Wide310x150Logo.scale-$scale.png"
    Write-Logo ([Math]::Round(310 * $at)) ([Math]::Round(310 * $at)) 0.6 "Square310x310Logo.scale-$scale.png"
}
# The taskbar's and Start's list: unplated, so Windows draws no square behind the icon.
foreach ($target in 16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 256) {
    foreach ($form in '', '_altform-unplated', '_altform-lightunplated') {
        Write-Logo $target $target 1 "Square44x44Logo.targetsize-$target$form.png"
    }
}
$icon.Dispose()
