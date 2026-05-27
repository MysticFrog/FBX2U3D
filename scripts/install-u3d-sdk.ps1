param(
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA 'fbx2u3d\u3d-sdk')
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$releaseName = 'U3D_A_061228_5'
$downloadPageUrl = 'https://sourceforge.net/projects/u3d/files/Universal%203D%20Sample%20Software/Gold%20Update%201.2/U3D_A_061228_5.zip/download'
$vendoredArchivePath = Join-Path $repoRoot "third_party\u3d-sdk\$releaseName.zip"
$scratchRoot = Join-Path $env:TEMP 'fbx2u3d-u3d-sdk'
$wrapperPath = Join-Path $scratchRoot 'download.html'
$archivePath = Join-Path $scratchRoot "$releaseName.zip"
$extractPath = Join-Path $scratchRoot 'extract'
$destinationPath = Join-Path $InstallRoot $releaseName

New-Item -ItemType Directory -Force -Path $scratchRoot | Out-Null
New-Item -ItemType Directory -Force -Path $InstallRoot | Out-Null

if (Test-Path -LiteralPath $vendoredArchivePath) {
    Copy-Item -LiteralPath $vendoredArchivePath -Destination $archivePath -Force
    Write-Output "Using vendored SDK archive: $vendoredArchivePath"
}
else {
    Invoke-WebRequest -Uri $downloadPageUrl -OutFile $wrapperPath
    $wrapperContent = Get-Content -LiteralPath $wrapperPath -Raw
    $mirrorMatch = [regex]::Match(
        $wrapperContent,
        'https://downloads\.sourceforge\.net/project/u3d/Universal%203D%20Sample%20Software/Gold%20Update%201\.2/U3D_A_061228_5\.zip\?[^"'']+'
    )

    if (-not $mirrorMatch.Success) {
        throw 'Could not resolve the SourceForge mirror URL for the U3D SDK.'
    }

    $mirrorUrl = $mirrorMatch.Value -replace '&amp;', '&'
    Invoke-WebRequest -Uri $mirrorUrl -OutFile $archivePath
}

if (Test-Path $extractPath) {
    Remove-Item -Recurse -Force $extractPath
}

Expand-Archive -LiteralPath $archivePath -DestinationPath $extractPath

if (Test-Path $destinationPath) {
    Remove-Item -Recurse -Force $destinationPath
}

Copy-Item -Recurse -Force (Join-Path $extractPath $releaseName) $destinationPath

$converterPath = Join-Path $destinationPath 'Bin\Win32\Release\IDTFConverter.exe'

if (-not (Test-Path $converterPath)) {
    throw "IDTFConverter.exe was not found after extraction: $converterPath"
}

Write-Output "Installed IDTFConverter.exe to $converterPath"