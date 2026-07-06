# Generates assets/devterm.ico from scratch (no external tools).
#
# Draws a rounded dark terminal tile with a green ">_" prompt, renders it at several
# resolutions, and packs them into a multi-size .ico (PNG-compressed entries, Vista+).
# Re-run this whenever the icon design changes.  Requires Windows + .NET (System.Drawing).

Add-Type -AssemblyName System.Drawing

$OutDir = Join-Path $PSScriptRoot '..\assets'
$OutIco = Join-Path $OutDir 'devterm.ico'
$OutPng = Join-Path $OutDir 'devterm-256.png'   # runtime winit window/taskbar icon
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function New-IconBitmap([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode     = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAlias
    $g.Clear([System.Drawing.Color]::Transparent)

    # Rounded background tile.
    $pad    = [int]($size * 0.06)
    $radius = [int]($size * 0.18)
    $rect   = New-Object System.Drawing.Rectangle($pad, $pad, ($size - 2*$pad), ($size - 2*$pad))
    $path   = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d      = $radius * 2
    $path.AddArc($rect.X, $rect.Y, $d, $d, 180, 90)
    $path.AddArc($rect.Right - $d, $rect.Y, $d, $d, 270, 90)
    $path.AddArc($rect.Right - $d, $rect.Bottom - $d, $d, $d, 0, 90)
    $path.AddArc($rect.X, $rect.Bottom - $d, $d, $d, 90, 90)
    $path.CloseFigure()

    $bg = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        $rect,
        [System.Drawing.Color]::FromArgb(255, 30, 34, 42),
        [System.Drawing.Color]::FromArgb(255, 17, 19, 24),
        [System.Drawing.Drawing2D.LinearGradientMode]::ForwardDiagonal)
    $g.FillPath($bg, $path)

    # Subtle border.
    $pen = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(60, 255, 255, 255), [Math]::Max(1, $size * 0.01))
    $g.DrawPath($pen, $path)

    # ">_" prompt glyphs, drawn as strokes so they stay crisp at any size.
    $stroke = [Math]::Max(2, $size * 0.07)
    $green  = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(255, 63, 214, 122), $stroke)
    $green.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $green.EndCap   = [System.Drawing.Drawing2D.LineCap]::Round
    $green.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round

    $cx = $size * 0.34
    $cy = $size * 0.44
    $arm = $size * 0.11
    # chevron ">"
    [System.Drawing.PointF[]]$chevron = @(
        (New-Object System.Drawing.PointF([single]($cx - $arm), [single]($cy - $arm))),
        (New-Object System.Drawing.PointF([single]($cx + $arm), [single]$cy)),
        (New-Object System.Drawing.PointF([single]($cx - $arm), [single]($cy + $arm)))
    )
    $g.DrawLines($green, $chevron)
    # underscore "_"
    $white = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(255, 235, 238, 245), $stroke)
    $white.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $white.EndCap   = [System.Drawing.Drawing2D.LineCap]::Round
    $uy = $size * 0.58
    $g.DrawLine($white, [single]($size * 0.50), [single]$uy, [single]($size * 0.68), [single]$uy)

    $g.Dispose()
    return $bmp
}

# Sizes to embed. 256 uses PNG compression; all entries are stored as PNG (valid on Vista+).
$sizes = 256, 128, 64, 48, 32, 16
$pngs  = @()
foreach ($s in $sizes) {
    $bmp = New-IconBitmap $s
    $ms  = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $pngs += ,($ms.ToArray())
    if ($s -eq 256) { [System.IO.File]::WriteAllBytes($OutPng, $ms.ToArray()) }
    $bmp.Dispose()
    $ms.Dispose()
}

# --- Assemble the ICO container -------------------------------------------------
$fs = [System.IO.File]::Create($OutIco)
$bw = New-Object System.IO.BinaryWriter($fs)
$bw.Write([UInt16]0)              # reserved
$bw.Write([UInt16]1)              # type: 1 = icon
$bw.Write([UInt16]$sizes.Count)   # image count

$offset = 6 + (16 * $sizes.Count) # header + directory entries
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i]
    $b = if ($s -ge 256) { 0 } else { $s }   # 0 means 256 in ICO dir
    $bw.Write([Byte]$b)                       # width
    $bw.Write([Byte]$b)                       # height
    $bw.Write([Byte]0)                        # palette count
    $bw.Write([Byte]0)                        # reserved
    $bw.Write([UInt16]1)                      # color planes
    $bw.Write([UInt16]32)                     # bits per pixel
    $bw.Write([UInt32]$pngs[$i].Length)       # image size
    $bw.Write([UInt32]$offset)                # image offset
    $offset += $pngs[$i].Length
}
foreach ($png in $pngs) { $bw.Write($png) }
$bw.Flush(); $bw.Close(); $fs.Close()

Write-Host "Wrote $OutIco ($((Get-Item $OutIco).Length) bytes, sizes: $($sizes -join ', '))"
Write-Host "Wrote $OutPng ($((Get-Item $OutPng).Length) bytes, 256x256)"
