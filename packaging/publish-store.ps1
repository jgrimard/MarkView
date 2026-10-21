<#
.SYNOPSIS
    Publishes a MarkView update to the Microsoft Store through the Store submission API.

.DESCRIPTION
    Creates a submission (or reuses the pending one), replaces the package with
    dist\MarkView_<version>_x64.msix, and sets "What's new in this version" for every listing language from
    packaging\store\release-notes.json (language code -> text). Everything else (description, screenshots,
    pricing, ...) is carried over unchanged from the current submission.

    Screenshots are put in the same order in every language ($ScreenshotOrder, by file name), because
    the order drifts between languages when listings are edited in Partner Center or imported as CSV.

    Without -Commit the submission is left as a draft to review in Partner Center. With -Commit it is
    submitted for certification and goes live when certification passes. -ListingOnly keeps the
    current package and updates only the listings.

    Credentials come from the user environment variables MSSTORE_TENANT_ID, MSSTORE_CLIENT_ID and
    MSSTORE_CLIENT_SECRET (a Microsoft Entra application with the Manager role in Partner Center, under
    Account settings > User management > Microsoft Entra applications).

.EXAMPLE
    .\packaging\publish-store.ps1
    .\packaging\publish-store.ps1 -Commit
    .\packaging\publish-store.ps1 -ListingOnly -Commit
#>
#Requires -Version 7
param(
    [string] $Package,
    [string] $ReleaseNotes,
    [switch] $ListingOnly,
    [switch] $Commit
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$appId = '9P0671Z6HDCP'
$ScreenshotOrder = 'screenshot-light.png', 'screenshot-dark-editor.png', 'screenshot-dark-readme.png', 'screenshot-tabs.png'
$api = "https://manage.devcenter.microsoft.com/v1.0/my/applications/$appId"

function Step($message) { Write-Host "==> $message" -ForegroundColor Cyan }
function Setting($name) {
    $v = [Environment]::GetEnvironmentVariable($name) ?? [Environment]::GetEnvironmentVariable($name, 'User')
    if (-not $v) { throw "$name is not set" }
    $v
}

if (-not $ListingOnly) {
    if (-not $Package) {
        $cargoToml = Get-Content (Join-Path $root 'src-tauri\Cargo.toml') -Raw
        if ($cargoToml -notmatch '(?m)^version\s*=\s*"(\d+\.\d+\.\d+)"') { throw 'No a.b.c version in Cargo.toml' }
        $Package = Join-Path $root "dist\MarkView_$($Matches[1]).0_x64.msix"
    }
    if (-not (Test-Path $Package)) { throw "Package not found: $Package (run build-msix.ps1 first)" }
}
if (-not $ReleaseNotes) { $ReleaseNotes = Join-Path $root 'packaging\store\release-notes.json' }
$notes = Get-Content $ReleaseNotes -Raw -Encoding utf8 | ConvertFrom-Json -AsHashtable

# ---------------------------------------------------------------- sign in

Step 'Signing in'
$tenant = Setting 'MSSTORE_TENANT_ID'
$token = (Invoke-RestMethod -Method Post -Uri "https://login.microsoftonline.com/$tenant/oauth2/token" -Body @{
    grant_type = 'client_credentials'
    client_id = Setting 'MSSTORE_CLIENT_ID'
    client_secret = Setting 'MSSTORE_CLIENT_SECRET'
    resource = 'https://manage.devcenter.microsoft.com'
}).access_token
$headers = @{ Authorization = "Bearer $token" }
function Call($method, $path, $body) {
    $request = @{ Method = $method; Uri = "$api$path"; Headers = $headers; ContentType = 'application/json; charset=utf-8' }
    if ($null -ne $body) { $request.Body = [Text.Encoding]::UTF8.GetBytes(($body | ConvertTo-Json -Depth 30)) }
    Invoke-RestMethod @request
}

# ---------------------------------------------------------------- submission

$app = Call Get ''
if ($app.pendingApplicationSubmission) {
    Step "Reusing pending submission $($app.pendingApplicationSubmission.id)"
    $submission = Call Get "/submissions/$($app.pendingApplicationSubmission.id)"
} else {
    Step 'Creating a submission from the published one'
    $submission = Call Post '/submissions'
}
$id = $submission.id

# ---------------------------------------------------------------- package

if (-not $ListingOnly) {
    $fileName = Split-Path $Package -Leaf
    Step "Replacing the package with $fileName"
    foreach ($p in $submission.applicationPackages) {
        if ($p.fileName -ne $fileName) { $p.fileStatus = 'PendingDelete' }
    }
    if (-not ($submission.applicationPackages | Where-Object fileName -eq $fileName)) {
        $submission.applicationPackages += [pscustomobject]@{ fileName = $fileName; fileStatus = 'PendingUpload' }
    }
}

# ---------------------------------------------------------------- listings

Step "Setting release notes and screenshot order for $(@($submission.listings.PSObject.Properties).Count) languages"
foreach ($listing in $submission.listings.PSObject.Properties) {
    $base = $listing.Value.baseListing
    $text = $notes[$listing.Name.ToLowerInvariant()]
    if (-not $text) { throw "No release notes for $($listing.Name) in $ReleaseNotes" }
    if ($text.Length -gt 1500) { throw "Release notes for $($listing.Name) are $($text.Length) characters (max 1500)" }
    $base.releaseNotes = $text

    $shots = @($base.images | Where-Object imageType -eq 'Screenshot')
    $unknown = $shots | Where-Object { $_.fileName -notin $ScreenshotOrder }
    if ($unknown) { throw "$($listing.Name) has screenshots not in `$ScreenshotOrder: $($unknown.fileName -join ', ')" }
    $sorted = $shots | Sort-Object { [array]::IndexOf($ScreenshotOrder, $_.fileName) }
    $base.images = @($sorted) + @($base.images | Where-Object imageType -ne 'Screenshot')
}

# ---------------------------------------------------------------- upload and save

if (-not $ListingOnly) {
    $zip = Join-Path ([IO.Path]::GetTempPath()) "MarkView-store-$id.zip"
    Compress-Archive -Path $Package -DestinationPath $zip -Force
    Step "Uploading $fileName ($([math]::Round((Get-Item $zip).Length / 1MB, 1)) MB)"
    Invoke-RestMethod -Method Put -Uri $submission.fileUploadUrl -InFile $zip -Headers @{ 'x-ms-blob-type' = 'BlockBlob' }
    Remove-Item $zip
}

Step 'Saving the submission'
$null = Call Put "/submissions/$id" $submission

if (-not $Commit) {
    Write-Host "Draft saved: https://partner.microsoft.com/dashboard/products/$appId/submissions/$id"
    Write-Host 'Run again with -Commit to submit it for certification.'
    return
}

# ---------------------------------------------------------------- commit

Step 'Submitting for certification'
$null = Call Post "/submissions/$id/commit"
do {
    Start-Sleep -Seconds 10
    $status = Call Get "/submissions/$id/status"
    Write-Host "    status: $($status.status)"
} while ($status.status -eq 'CommitStarted')
if ($status.status -eq 'CommitFailed') {
    $status.statusDetails.errors | ForEach-Object { Write-Host "    error: $($_.code) $($_.details)" -ForegroundColor Red }
    throw 'Commit failed'
}
Write-Host "Submitted. Track certification at https://partner.microsoft.com/dashboard/products/$appId/overview"
