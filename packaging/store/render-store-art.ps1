<#
.SYNOPSIS
    Renders the Microsoft Store listing logos from render.html (and docs\images\logo.svg) with headless Edge.

.DESCRIPTION
    Writes into this folder:
      box-art-2160.png       1:1 Box art (2160 x 2160)
      poster-art-1440x2160.png  9:16 Poster art (1440 x 2160)
      tile-icon-300.png      1:1 App tile icon (300 x 300)
      hero-1920x1080.png     16:9 Super hero art (1920 x 1080)
#>
$ErrorActionPreference = 'Stop'

$edge = @("${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe",
          "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $edge) { throw 'Microsoft Edge not found.' }

$page = ([Uri](Join-Path $PSScriptRoot 'render.html')).AbsoluteUri
$profile = Join-Path ([IO.Path]::GetTempPath()) 'markview-store-art'

$art = @(
    @{ Name = 'box-art-2160.png'; Art = 'icon'; Width = 2160; Height = 2160 },
    @{ Name = 'poster-art-1440x2160.png'; Art = 'poster'; Width = 1440; Height = 2160 },
    @{ Name = 'tile-icon-300.png'; Art = 'icon'; Width = 300; Height = 300 },
    @{ Name = 'hero-1920x1080.png'; Art = 'hero'; Width = 1920; Height = 1080 }
)

Add-Type -AssemblyName System.Drawing

foreach ($a in $art) {
    $out = Join-Path $PSScriptRoot $a.Name
    Write-Host "==> $($a.Name)"
    # Headless Edge won't size a window below ~500 px wide, so small art is rendered at 4x and scaled down.
    $scale = if ($a.Width -lt 1000) { 4 } else { 1 }
    $shot = if ($scale -gt 1) { "$out.tmp.png" } else { $out }
    if (Test-Path $shot) { Remove-Item $shot }
    # Edge reports progress on stderr, which Windows PowerShell 5.1 would otherwise treat as a terminating error.
    $ErrorActionPreference = 'Continue'
    & $edge --headless=new --disable-gpu --hide-scrollbars --default-background-color=00000000 `
        "--user-data-dir=$profile" "--window-size=$($a.Width * $scale),$($a.Height * $scale)" "--screenshot=$shot" `
        "$page`?art=$($a.Art)" 2>&1 | Out-Null
    $ErrorActionPreference = 'Stop'
    if (-not (Test-Path $shot)) { throw "Edge did not write $shot" }
    if ($scale -gt 1) {
        $big = [System.Drawing.Image]::FromFile($shot)
        $small = New-Object System.Drawing.Bitmap $a.Width, $a.Height
        $g = [System.Drawing.Graphics]::FromImage($small)
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $g.DrawImage($big, 0, 0, $a.Width, $a.Height)
        $g.Dispose(); $big.Dispose()
        $small.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
        $small.Dispose()
        Remove-Item $shot
    }
}
Remove-Item $profile -Recurse -Force -ErrorAction SilentlyContinue
