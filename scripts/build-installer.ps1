param(
    [string]$Configuration = 'release',
    [string]$InnoSetupRoot = 'C:\Users\ajones\AppData\Local\Programs\Inno Setup 6'
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$cargoExe = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
$isccExe = Join-Path $InnoSetupRoot 'ISCC.exe'
$installedSdkRoot = Join-Path $env:LOCALAPPDATA 'fbx2u3d\u3d-sdk\U3D_A_061228_5\Bin\Win32\Release'
$vendoredSdkArchive = Join-Path $repoRoot 'third_party\u3d-sdk\U3D_A_061228_5.zip'
$vendoredSdkExtractRoot = Join-Path $env:TEMP 'fbx2u3d-vendored-sdk'
$sdkRoot = $installedSdkRoot
$stagingRoot = Join-Path $repoRoot 'dist\staging'
$stagingAppRoot = Join-Path $stagingRoot 'FBX2U3D'
$cargoConfigPath = Join-Path $repoRoot '.cargo\config.toml'
$distRoot = Join-Path $repoRoot 'dist'
$tempOutputRoot = Join-Path $env:TEMP 'fbx2u3d-installer-output'
$targetRoot = Join-Path $repoRoot 'target'

if (Test-Path -LiteralPath $cargoConfigPath) {
    $cargoConfig = Get-Content -LiteralPath $cargoConfigPath -Raw
    $targetMatch = [regex]::Match($cargoConfig, 'target-dir\s*=\s*"([^"]+)"')
    if ($targetMatch.Success) {
        $targetRoot = [System.IO.Path]::GetFullPath($targetMatch.Groups[1].Value)
    }
}

$releaseExe = Join-Path $targetRoot "$Configuration\fbx2u3d.exe"

if (-not (Test-Path -LiteralPath $cargoExe)) {
    throw "Cargo was not found: $cargoExe"
}

if (-not (Test-Path -LiteralPath $isccExe)) {
    throw "Inno Setup compiler was not found: $isccExe"
}

if (Test-Path -LiteralPath $vendoredSdkArchive) {
    if (Test-Path -LiteralPath $vendoredSdkExtractRoot) {
        Remove-Item -LiteralPath $vendoredSdkExtractRoot -Recurse -Force
    }

    Expand-Archive -LiteralPath $vendoredSdkArchive -DestinationPath $vendoredSdkExtractRoot -Force
    $sdkRoot = Join-Path $vendoredSdkExtractRoot 'U3D_A_061228_5\Bin\Win32\Release'
}

if (-not (Test-Path -LiteralPath $sdkRoot)) {
    throw "Bundled SDK runtime was not found: $sdkRoot"
}

$env:PATH = 'C:\msys64\mingw64\bin;' + (Join-Path $env:USERPROFILE '.cargo\bin;') + $env:PATH

Push-Location $repoRoot
try {
    & $cargoExe build --release
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release failed with exit code $LASTEXITCODE"
    }

    if (Test-Path -LiteralPath $stagingRoot) {
        Remove-Item -LiteralPath $stagingRoot -Recurse -Force
    }

    if (Test-Path -LiteralPath $tempOutputRoot) {
        Remove-Item -LiteralPath $tempOutputRoot -Recurse -Force
    }

    New-Item -ItemType Directory -Path $stagingAppRoot | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $stagingAppRoot 'u3d-sdk\U3D_A_061228_5\Bin\Win32\Release') -Force | Out-Null
    New-Item -ItemType Directory -Path $distRoot -Force | Out-Null
    New-Item -ItemType Directory -Path $tempOutputRoot -Force | Out-Null

    Copy-Item -LiteralPath $releaseExe -Destination (Join-Path $stagingAppRoot 'fbx2u3d.exe')
    Copy-Item -LiteralPath (Join-Path $repoRoot 'installer\quick-convert.ps1') -Destination (Join-Path $stagingAppRoot 'quick-convert.ps1')
    Copy-Item -LiteralPath (Join-Path $repoRoot 'installer\quick-convert.vbs') -Destination (Join-Path $stagingAppRoot 'quick-convert.vbs')
    Copy-Item -LiteralPath (Join-Path $repoRoot 'FBX2U3D.png') -Destination (Join-Path $stagingAppRoot 'FBX2U3D.png')
    Copy-Item -LiteralPath (Join-Path $repoRoot 'README.md') -Destination (Join-Path $stagingAppRoot 'README.md')
    Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE') -Destination (Join-Path $stagingAppRoot 'LICENSE')
    Copy-Item -LiteralPath (Join-Path $repoRoot 'THIRD_PARTY_NOTICES.md') -Destination (Join-Path $stagingAppRoot 'THIRD_PARTY_NOTICES.md')
    Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE_COMPATIBILITY.md') -Destination (Join-Path $stagingAppRoot 'LICENSE_COMPATIBILITY.md')
    Copy-Item -LiteralPath (Join-Path $repoRoot 'third_party\u3d-sdk\LICENSE-APACHE-2.0.txt') -Destination (Join-Path $stagingAppRoot 'Intel-U3D-SDK-LICENSE-APACHE-2.0.txt')
    Get-ChildItem -LiteralPath $sdkRoot | Copy-Item -Destination (Join-Path $stagingAppRoot 'u3d-sdk\U3D_A_061228_5\Bin\Win32\Release') -Recurse -Force

    $stagedConverterPath = Join-Path $stagingAppRoot 'u3d-sdk\U3D_A_061228_5\Bin\Win32\Release\IDTFConverter.exe'
    if (-not (Test-Path -LiteralPath $stagedConverterPath)) {
        throw "Staged SDK runtime is missing IDTFConverter.exe: $stagedConverterPath"
    }

    $cargoToml = Get-Content -LiteralPath (Join-Path $repoRoot 'Cargo.toml') -Raw
    $versionMatch = [regex]::Match($cargoToml, 'version\s*=\s*"([^"]+)"')
    if (-not $versionMatch.Success) {
        throw 'Could not determine package version from Cargo.toml.'
    }

    & $isccExe "/O$tempOutputRoot" "/DMyAppVersion=$($versionMatch.Groups[1].Value)" "/DStagingDir=$stagingAppRoot" (Join-Path $repoRoot 'installer\FBX2U3D.iss')
    if ($LASTEXITCODE -ne 0) {
        throw "ISCC.exe failed with exit code $LASTEXITCODE"
    }

    Copy-Item -Path (Join-Path $tempOutputRoot '*') -Destination $distRoot -Force

    Write-Output "Installer created under $distRoot"
}
finally {
    Pop-Location
}