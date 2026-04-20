# Generates packaging\windows\oscan.ico from the runtime's procedural default icon.
# The pixel formula mirrors osc_build_default_icon() in runtime/osc_runtime.c so the
# MSI / ARP icon matches the "retro phosphor-green O" shown by the canvas builtins.
#
# Usage:
#   pwsh -File scripts\gen-oscan-ico.ps1
#
# Produces:
#   packaging\windows\oscan.ico  (multi-size: 16, 32, 48, 256)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$OutPath  = Join-Path $RepoRoot "packaging\windows\oscan.ico"

Add-Type -AssemblyName System.Drawing

# Same 16x16 glyph as runtime/osc_runtime.c (MSB = leftmost pixel).
$glyph = @(
    0x0000, 0x07E0, 0x1FF8, 0x3C3C,
    0x381C, 0x700E, 0x700E, 0x700E,
    0x700E, 0x700E, 0x700E, 0x381C,
    0x3C3C, 0x1FF8, 0x07E0, 0x0000
)

function New-OscanBitmap([int]$N) {
    $bmp = New-Object System.Drawing.Bitmap $N, $N, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    # Scale 16x16 glyph up to N using integer nearest-neighbor.
    for ($y = 0; $y -lt $N; $y++) {
        for ($x = 0; $x -lt $N; $x++) {
            $gy = [int]([math]::Floor($y * 16 / $N))
            $gx = [int]([math]::Floor($x * 16 / $N))
            $on = (($glyph[$gy] -shr (15 - $gx)) -band 1) -eq 1
            $thick = [int][math]::Max(1, [math]::Round($N / 16))
            $border = ($x -lt $thick) -or ($y -lt $thick) -or ($x -ge $N - $thick) -or ($y -ge $N - $thick)
            if ($on) {
                $g = if ($y % 2 -eq 1) { 0xC8 } else { 0xFF }
                $c = [System.Drawing.Color]::FromArgb(0xFF, 0x10, $g, 0x20)
            } elseif ($border) {
                $c = [System.Drawing.Color]::FromArgb(0xFF, 0xB0, 0xB0, 0xB0)
            } else {
                $c = [System.Drawing.Color]::FromArgb(0xFF, 0x08, 0x0C, 0x08)
            }
            $bmp.SetPixel($x, $y, $c)
        }
    }
    return $bmp
}

function Get-PngBytes([System.Drawing.Bitmap]$bmp) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    return $ms.ToArray()
}

$sizes = @(16, 32, 48, 256)
$pngs  = New-Object 'System.Collections.Generic.List[byte[]]'
foreach ($s in $sizes) {
    $bmp = New-OscanBitmap $s
    [void]$pngs.Add((Get-PngBytes $bmp))
    $bmp.Dispose()
}

# Write ICO file (ICONDIR + ICONDIRENTRYs + PNG payloads).
$fs = [System.IO.File]::Create($OutPath)
$bw = New-Object System.IO.BinaryWriter($fs)
try {
    $bw.Write([uint16]0)            # reserved
    $bw.Write([uint16]1)            # type = icon
    $bw.Write([uint16]$sizes.Count) # image count

    $headerSize = 6 + (16 * $sizes.Count)
    $offset = $headerSize
    for ($i = 0; $i -lt $sizes.Count; $i++) {
        $s = $sizes[$i]
        $w = if ($s -ge 256) { 0 } else { [byte]$s }
        $h = if ($s -ge 256) { 0 } else { [byte]$s }
        $bw.Write([byte]$w)         # width
        $bw.Write([byte]$h)         # height
        $bw.Write([byte]0)          # colors in palette
        $bw.Write([byte]0)          # reserved
        $bw.Write([uint16]1)        # planes
        $bw.Write([uint16]32)       # bpp
        $bw.Write([uint32]$pngs[$i].Length) # bytes in resource
        $bw.Write([uint32]$offset)  # offset
        $offset += $pngs[$i].Length
    }
    foreach ($p in $pngs) { $bw.Write([byte[]]$p) }
}
finally {
    $bw.Dispose()
    $fs.Dispose()
}

Write-Host "Wrote $OutPath ($((Get-Item $OutPath).Length) bytes)"
