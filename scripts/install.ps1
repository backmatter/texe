param(
    [string]$From,
    [string]$Prefix = "$env:LOCALAPPDATA\Programs\texe"
)

$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($Prefix)) {
    throw "Refusing unsafe install prefix: $Prefix"
}
$Prefix = [System.IO.Path]::GetFullPath($Prefix)
if ($Prefix -eq [System.IO.Path]::GetPathRoot($Prefix)) {
    throw "Refusing unsafe install prefix: $Prefix"
}

$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
if ($architecture -ne [System.Runtime.InteropServices.Architecture]::X64) {
    throw "texe supports Windows x86-64. This computer reports $architecture."
}

$archiveName = "texe-x86_64-windows.zip"
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("texe-install-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $work | Out-Null
try {
    if ([string]::IsNullOrWhiteSpace($From)) {
        $releaseBase = $env:TEXE_RELEASE_BASE_URL
        if ([string]::IsNullOrWhiteSpace($releaseBase)) {
            $releaseBase = "https://github.com/backmatter/texe/releases/latest/download"
        }
        if ($releaseBase.StartsWith("http://")) {
            if ($env:TEXE_INSTALL_TEST_ALLOW_HTTP -ne "1") {
                throw "Refusing a non-HTTPS release URL"
            }
        }
        elseif (-not $releaseBase.StartsWith("https://")) {
            throw "Release URL must use HTTPS"
        }

        $From = Join-Path $work $archiveName
        $checksums = Join-Path $work "SHA256SUMS"
        Write-Host "Downloading the latest texe command suite"
        Invoke-WebRequest "$releaseBase/$archiveName" -OutFile $From -UseBasicParsing
        Invoke-WebRequest "$releaseBase/SHA256SUMS" -OutFile $checksums -UseBasicParsing

        $expected = $null
        foreach ($line in Get-Content $checksums) {
            if ($line -match "^([0-9a-fA-F]{64})\s+\*?(.+)$") {
                if ($Matches[2].Trim() -eq $archiveName) {
                    $expected = $Matches[1].ToLowerInvariant()
                    break
                }
            }
        }
        if (-not $expected) {
            throw "Release checksum is invalid"
        }
        $actual = (Get-FileHash -Algorithm SHA256 $From).Hash.ToLowerInvariant()
        if ($actual -ne $expected) {
            throw "Downloaded archive failed checksum verification"
        }
    }
    elseif (-not (Test-Path -LiteralPath $From -PathType Leaf)) {
        throw "Archive not found: $From"
    }

    $extracted = Join-Path $work "extracted"
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead((Resolve-Path -LiteralPath $From))
    try {
        $root = [System.IO.Path]::GetFullPath($extracted) + [System.IO.Path]::DirectorySeparatorChar
        foreach ($entry in $zip.Entries) {
            $relative = $entry.FullName.Replace('/', [System.IO.Path]::DirectorySeparatorChar)
            $destination = [System.IO.Path]::GetFullPath((Join-Path $extracted $relative))
            if (-not $destination.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw "Archive contains unsafe path: $($entry.FullName)"
            }
            $unixMode = ($entry.ExternalAttributes -shr 16) -band 0xF000
            if ($unixMode -eq 0xA000) {
                throw "Archive contains an unsupported symbolic link: $($entry.FullName)"
            }
        }
    }
    finally {
        $zip.Dispose()
    }
    Expand-Archive -LiteralPath $From -DestinationPath $extracted
    $bundle = Join-Path $extracted "texe-x86_64-windows"
    if (-not (Test-Path -LiteralPath $bundle -PathType Container)) {
        throw "Archive does not contain the expected command suite"
    }
    $expectedFiles = @{}
    Get-Content (Join-Path $bundle "SHA256SUMS") | ForEach-Object {
        if ($_ -match '^([0-9a-f]{64})\s+\*?(.+)$') {
            $expectedFiles[$Matches[2].Replace('/', '\')] = $Matches[1]
        }
    }
    foreach ($relative in @("bin\texe.exe", "bin\pqty.exe", "bin\pqty-fls.exe")) {
        $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $bundle $relative)).Hash.ToLowerInvariant()
        if ($actual -ne $expectedFiles[$relative]) {
            throw "Checksum verification failed for $relative"
        }
    }

    $bin = Join-Path $Prefix "bin"
    New-Item -ItemType Directory -Force -Path $bin | Out-Null
    foreach ($binary in @("texe.exe", "pqty.exe", "pqty-fls.exe")) {
        Copy-Item -Force (Join-Path $bundle "bin\$binary") (Join-Path $bin $binary)
    }
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $parts = @($userPath -split ';' | Where-Object { $_ })
    if ($parts -notcontains $bin) {
        [Environment]::SetEnvironmentVariable("Path", (($parts + $bin) -join ';'), "User")
    }
    Write-Host "Installed texe, pqty, and pqty-fls to $bin"
    Write-Host "Open a new PowerShell window, then run: texe"
}
finally {
    if ($work -like (Join-Path ([System.IO.Path]::GetTempPath()) "texe-install-*")) {
        Remove-Item -LiteralPath $work -Recurse -Force
    }
}
