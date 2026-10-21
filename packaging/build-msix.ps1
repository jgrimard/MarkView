<#
.SYNOPSIS
    Builds the Microsoft Store package (MSIX) for MarkView.

.DESCRIPTION
    Builds the release exe, lays out the package (manifest, exe, logo assets, localized strings, resources.pri) under
    dist\msix\<arch>, and packs it into dist\MarkView_<version>_<arch>.msix. For Partner Center it also writes
    dist\MarkView_<version>_<arch>.msixupload, the .msix together with its debug symbols, so the Store's crash
    reports show call stacks. The Store signs the package itself, so this script does not.

    Needs the Windows 10/11 SDK (makeappx.exe, makepri.exe) and Rust. Works in PowerShell 5.1 and 7.

.PARAMETER Arch
    x64 (default) or arm64. arm64 needs: rustup target add aarch64-pc-windows-msvc

.PARAMETER Install
    Also register the unpacked layout for the current user (needs Developer Mode), to try the packaged
    build locally. Remove it again with: Get-AppxPackage jgrimard.388874D7F8808 | Remove-AppxPackage

.EXAMPLE
    .\packaging\build-msix.ps1
    .\packaging\build-msix.ps1 -Arch arm64
    .\packaging\build-msix.ps1 -Install
#>
param(
    [ValidateSet('x64', 'arm64')] [string] $Arch = 'x64',
    [switch] $Install
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$tauri = Join-Path $root 'src-tauri'
$layout = Join-Path $root "dist\msix\$Arch"

function Step($message) { Write-Host "==> $message" -ForegroundColor Cyan }

# ---------------------------------------------------------------- SDK tools

$sdkBin = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\10.*" -Directory -ErrorAction SilentlyContinue |
    Sort-Object { [version]$_.Name } -Descending |
    Where-Object { Test-Path (Join-Path $_.FullName 'x64\makeappx.exe') } |
    Select-Object -First 1
if (-not $sdkBin) { throw 'makeappx.exe not found. Install the Windows SDK (Visual Studio Installer > Individual components).' }
$makeappx = Join-Path $sdkBin.FullName 'x64\makeappx.exe'
$makepri = Join-Path $sdkBin.FullName 'x64\makepri.exe'

# ---------------------------------------------------------------- version (Store needs a.b.c.0)

$cargoToml = Get-Content (Join-Path $tauri 'Cargo.toml') -Raw
if ($cargoToml -notmatch '(?m)^version\s*=\s*"(\d+)\.(\d+)\.(\d+)"') { throw 'No a.b.c version in Cargo.toml' }
$version = "$($Matches[1]).$($Matches[2]).$($Matches[3]).0"

# ---------------------------------------------------------------- exe

$target = @{ x64 = 'x86_64-pc-windows-msvc'; arm64 = 'aarch64-pc-windows-msvc' }[$Arch]
Step "Building markview.exe $version ($target)"
Push-Location $tauri
try {
    cargo build --release --target $target
    if ($LASTEXITCODE) { throw "cargo build failed ($LASTEXITCODE)" }
} finally { Pop-Location }
$exe = Join-Path $tauri "target\$target\release\markview.exe"

# ---------------------------------------------------------------- layout

Step "Laying out $layout"
if (Test-Path $layout) { Remove-Item $layout -Recurse -Force }
$assets = New-Item -ItemType Directory -Force (Join-Path $layout 'Assets')
Copy-Item $exe $layout

$utf8 = New-Object System.Text.UTF8Encoding($false)
$manifest = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'AppxManifest.xml'), $utf8)
$manifest = $manifest.Replace('{{VERSION}}', $version).Replace('{{ARCH}}', $Arch)
[IO.File]::WriteAllText((Join-Path $layout 'AppxManifest.xml'), $manifest, $utf8)

# Logos: scaled from the 512 px logo; taskbar/Start sizes taken straight from the hand-sized PNGs in icon.ico.
Add-Type -AssemblyName System.Drawing
$logo = [System.Drawing.Image]::FromFile((Join-Path $root 'docs\images\logo.png'))

function Save-Scaled($name, $width, $height) {
    $bitmap = New-Object System.Drawing.Bitmap $width, $height
    $g = [System.Drawing.Graphics]::FromImage($bitmap)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $size = [Math]::Min($width, $height)
    $g.DrawImage($logo, [int](($width - $size) / 2), [int](($height - $size) / 2), $size, $size)
    $g.Dispose()
    $bitmap.Save((Join-Path $assets $name), [System.Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
}

try {
    foreach ($scale in 100, 200, 400) {
        $f = $scale / 100
        Save-Scaled "Square44x44Logo.scale-$scale.png" (44 * $f) (44 * $f)
        Save-Scaled "Square150x150Logo.scale-$scale.png" (150 * $f) (150 * $f)
        Save-Scaled "StoreLogo.scale-$scale.png" (50 * $f) (50 * $f)
    }
} finally { $logo.Dispose() }

# icon.ico holds PNG frames; copy the ones Windows asks for by target size.
$ico = [IO.File]::ReadAllBytes((Join-Path $tauri 'icons\icon.ico'))
$count = [BitConverter]::ToUInt16($ico, 4)
for ($i = 0; $i -lt $count; $i++) {
    $entry = 6 + 16 * $i
    $px = if ($ico[$entry] -eq 0) { 256 } else { $ico[$entry] }
    $length = [BitConverter]::ToInt32($ico, $entry + 8)
    $offset = [BitConverter]::ToInt32($ico, $entry + 12)
    if ($px -notin 16, 24, 32, 48, 256) { continue }
    $png = New-Object byte[] $length
    [Array]::Copy($ico, $offset, $png, 0, $length)
    foreach ($suffix in '', '_altform-unplated', '_altform-lightunplated') {
        [IO.File]::WriteAllBytes((Join-Path $assets "Square44x44Logo.targetsize-$px$suffix.png"), $png)
    }
}

# The manifest's ms-resource: strings (app description, file type name), in every UI language.
$languages = @{
    'en' = 'en-US'; 'de' = 'de-DE'; 'es' = 'es-ES'; 'fr' = 'fr-FR'; 'it' = 'it-IT'; 'ja' = 'ja-JP'; 'ko' = 'ko-KR'
    'nl' = 'nl-NL'; 'pl' = 'pl-PL'; 'pt-BR' = 'pt-BR'; 'ru' = 'ru-RU'; 'tr' = 'tr-TR'; 'zh-Hans' = 'zh-CN'
}
# Reads one string from a locale file. (ConvertFrom-Json rejects keys differing only in case, such as
# CodeMirror's "cm.Replace" and "cm.replace", so only the value itself goes through it.)
function Get-LocaleString($json, $key) {
    $m = [regex]::Match($json, '"' + [regex]::Escape($key) + '"\s*:\s*("(?:[^"\\]|\\.)*")')
    if (-not $m.Success) { throw "$key is missing from a locale file" }
    $m.Groups[1].Value | ConvertFrom-Json
}

foreach ($id in $languages.Keys) {
    $json = [IO.File]::ReadAllText((Join-Path $root "ui\locales\$id.json"), $utf8)
    $data = @{
        AppDescription = Get-LocaleString $json 'assoc.description'
        FileTypeName = Get-LocaleString $json 'assoc.fileType'
    }
    $entries = foreach ($name in $data.Keys) {
        "  <data name=`"$name`" xml:space=`"preserve`"><value>$([Security.SecurityElement]::Escape($data[$name]))</value></data>"
    }
    $resw = "<?xml version=`"1.0`" encoding=`"utf-8`"?>`r`n<root>`r`n" +
        "  <resheader name=`"resmimetype`"><value>text/microsoft-resx</value></resheader>`r`n" +
        "  <resheader name=`"version`"><value>2.0</value></resheader>`r`n" +
        ($entries -join "`r`n") + "`r`n</root>`r`n"
    $dir = New-Item -ItemType Directory -Force (Join-Path $layout "Strings\$($languages[$id])")
    [IO.File]::WriteAllText((Join-Path $dir 'Resources.resw'), $resw, $utf8)
}

# resources.pri maps the plain names in the manifest to the scale/targetsize variants above, and the
# ms-resource: strings to the Strings\<language> folders.
Step 'Indexing resources'
$priConfig = Join-Path (Split-Path $layout) "priconfig-$Arch.xml"
& $makepri createconfig /cf $priConfig /dq en-US /pv 10.0.0 /o | Out-Null
if ($LASTEXITCODE) { throw "makepri createconfig failed ($LASTEXITCODE)" }
# One package, not a bundle: keep every scale in resources.pri instead of splitting out resource packs.
[xml]$config = Get-Content $priConfig -Raw
$packaging = $config.SelectSingleNode('//packaging')
if ($packaging) { [void]$packaging.ParentNode.RemoveChild($packaging) }
$config.Save($priConfig)
$output = & $makepri new /pr $layout /cf $priConfig /mn (Join-Path $layout 'AppxManifest.xml') /of (Join-Path $layout 'resources.pri') /o
if ($LASTEXITCODE) { $output; throw "makepri new failed ($LASTEXITCODE)" }
Remove-Item $priConfig

# ---------------------------------------------------------------- pack

$msix = Join-Path $root "dist\MarkView_${version}_$Arch.msix"
Step "Packing $msix"
$output = & $makeappx pack /d $layout /p $msix /o
if ($LASTEXITCODE) { $output; throw "makeappx pack failed ($LASTEXITCODE)" }

# ---------------------------------------------------------------- symbols

# Partner Center turns crash reports into call stacks when the PDB comes with the package: an .msixupload
# is a zip of the .msix and an .appxsym, which is a zip of the PDB.
$pdb = Join-Path $tauri "target\$target\release\markview.pdb"
if (-not (Test-Path $pdb)) { throw "$pdb not found (the release profile in Cargo.toml sets debug)" }
$appxsym = Join-Path $root "dist\MarkView_${version}_$Arch.appxsym"
$upload = Join-Path $root "dist\MarkView_${version}_$Arch.msixupload"
Step "Packing $upload"
# Compress-Archive only writes .zip names, so rename afterwards.
Compress-Archive -Path $pdb -DestinationPath "$appxsym.zip" -Force
Move-Item "$appxsym.zip" $appxsym -Force
Compress-Archive -Path $msix, $appxsym -DestinationPath "$upload.zip" -Force
Move-Item "$upload.zip" $upload -Force
Remove-Item $appxsym

if ($Install) {
    Step 'Registering the layout for this user'
    $name = ([xml]$manifest).Package.Identity.Name
    Get-AppxPackage -Name $name | Remove-AppxPackage
    Add-AppxPackage -Register (Join-Path $layout 'AppxManifest.xml')
}

Step "Done: $upload (upload this; it holds $(Split-Path $msix -Leaf) and its symbols)"
