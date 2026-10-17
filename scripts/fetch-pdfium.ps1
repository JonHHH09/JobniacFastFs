<#
.SYNOPSIS
    Downloads the pinned PDFium binary into vendor/pdfium/<platform>/ (Windows PowerShell 5.1 or PowerShell 7).

.DESCRIPTION
    Source: bblanchon/pdfium-binaries, release tag chromium/7881 (PDFium 151.0.7881.0). Each archive is
    checked against its pinned SHA-256 before it is extracted, and the extracted library is checked against
    its pinned SHA-256 before it is placed. Any mismatch fails closed: nothing is written under vendor/.

    Only the library, the upstream LICENSE and the licenses/ notice directory are kept. Binaries are never
    committed (vendor/ is gitignored). The fetch is idempotent: a platform whose library already matches the
    pinned SHA-256 is skipped.

    The code and tests find the library through the JOBNIAC_PDFIUM_DIR environment variable, which must point
    at vendor/pdfium/<platform>/ (the CI workflow exports it).

    Change the tag and the digests together.

.PARAMETER Platform
    windows-x64, linux-x64 or all (default).

.EXAMPLE
    pwsh -File scripts/fetch-pdfium.ps1 -Platform windows-x64
#>
[CmdletBinding()]
param(
    [ValidateSet('all', 'windows-x64', 'linux-x64')]
    [string]$Platform = 'all'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Pinned release. The release tag is part of the download URL: chromium/7881 is encoded as chromium%2F7881.
$ReleaseBaseUrl = 'https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881'

# Pinned digests (SHA-256, lower case). Archive digests were checked against the release asset metadata.
$Targets = @(
    @{
        Platform      = 'windows-x64'
        Archive       = 'pdfium-win-x64.tgz'
        ArchiveSha256 = '73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08'
        LibraryPath   = 'bin/pdfium.dll'
        LibrarySha256 = '79d4676b656cfb1abcea88f9ade3b4b0826c5200382db5f4ec72a636c598c118'
    },
    @{
        Platform      = 'linux-x64'
        Archive       = 'pdfium-linux-x64.tgz'
        ArchiveSha256 = '1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d'
        LibraryPath   = 'lib/libpdfium.so'
        LibrarySha256 = 'f728930966f503652b92acc89b9374a2eeca00ce42e26dccd3e4b5c5161b2d64'
    }
)

$RepoRoot = Split-Path -Parent $PSScriptRoot
$VendorRoot = Join-Path (Join-Path $RepoRoot 'vendor') 'pdfium'

function Get-Sha256Lower {
    param([Parameter(Mandatory)][string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Save-Download {
    param(
        [Parameter(Mandatory)][string]$Url,
        [Parameter(Mandatory)][string]$Destination
    )
    if ($PSVersionTable.PSEdition -eq 'Desktop') {
        # Windows PowerShell 5.1 defaults to TLS 1.0/1.1 for some hosts; GitHub requires TLS 1.2.
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    }
    Invoke-WebRequest -Uri $Url -OutFile $Destination -UseBasicParsing
}

# Prefer the Windows bsdtar. A Git for Windows GNU tar earlier on PATH reads "C:\..." as a remote host and fails.
$TarExe = Join-Path (Join-Path $env:SystemRoot 'System32') 'tar.exe'
if (-not (Test-Path -LiteralPath $TarExe -PathType Leaf)) {
    $tarCommand = Get-Command tar -ErrorAction SilentlyContinue
    if (-not $tarCommand) {
        throw 'tar is required to extract the PDFium archive (Windows 10 1803 or later ships tar.exe).'
    }
    $TarExe = $tarCommand.Source
}

foreach ($target in $Targets) {
    if ($Platform -ne 'all' -and $target.Platform -ne $Platform) {
        continue
    }

    $dest = Join-Path $VendorRoot $target.Platform
    $libraryLeaf = Split-Path -Leaf $target.LibraryPath
    $libraryInDest = Join-Path $dest $libraryLeaf

    if ((Test-Path -LiteralPath $libraryInDest -PathType Leaf) -and
        ((Get-Sha256Lower -Path $libraryInDest) -eq $target.LibrarySha256)) {
        Write-Host "[$($target.Platform)] present and verified: $libraryInDest"
        continue
    }

    $work = Join-Path ([IO.Path]::GetTempPath()) ('pdfium-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $work | Out-Null
    try {
        $archivePath = Join-Path $work $target.Archive
        $url = "$ReleaseBaseUrl/$($target.Archive)"
        Write-Host "[$($target.Platform)] downloading $($target.Archive)"
        Save-Download -Url $url -Destination $archivePath

        $archiveDigest = Get-Sha256Lower -Path $archivePath
        if ($archiveDigest -ne $target.ArchiveSha256) {
            throw "SHA-256 mismatch for $($target.Archive): expected $($target.ArchiveSha256), got $archiveDigest. Nothing was installed."
        }

        $extract = Join-Path $work 'extract'
        New-Item -ItemType Directory -Path $extract | Out-Null
        & $TarExe -xzf $archivePath -C $extract
        if ($LASTEXITCODE -ne 0) {
            throw "tar failed to extract $($target.Archive) (exit $LASTEXITCODE)."
        }

        $libraryInArchive = Join-Path $extract ($target.LibraryPath -replace '/', [IO.Path]::DirectorySeparatorChar)
        $licenseInArchive = Join-Path $extract 'LICENSE'
        $noticesInArchive = Join-Path $extract 'licenses'
        foreach ($required in @($libraryInArchive, $licenseInArchive, $noticesInArchive)) {
            if (-not (Test-Path -LiteralPath $required)) {
                throw "$($target.Archive) does not contain the expected member: $required"
            }
        }

        $libraryDigest = Get-Sha256Lower -Path $libraryInArchive
        if ($libraryDigest -ne $target.LibrarySha256) {
            throw "SHA-256 mismatch for $($target.LibraryPath) in $($target.Archive): expected $($target.LibrarySha256), got $libraryDigest. Nothing was installed."
        }

        # Stage the kept members first, then replace the platform directory in one step.
        $stage = Join-Path $work 'stage'
        New-Item -ItemType Directory -Path $stage | Out-Null
        Copy-Item -LiteralPath $libraryInArchive -Destination (Join-Path $stage $libraryLeaf)
        Copy-Item -LiteralPath $licenseInArchive -Destination (Join-Path $stage 'LICENSE')
        Copy-Item -LiteralPath $noticesInArchive -Destination (Join-Path $stage 'licenses') -Recurse

        if (Test-Path -LiteralPath $dest) {
            Remove-Item -LiteralPath $dest -Recurse -Force
        }
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest) | Out-Null
        Move-Item -LiteralPath $stage -Destination $dest
        Write-Host "[$($target.Platform)] installed $libraryInDest (sha256 $libraryDigest)"
    }
    finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}
