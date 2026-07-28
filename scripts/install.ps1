param(
    [Parameter(Mandatory = $true)]
    [string]$From,
    [string]$Prefix = "$env:LOCALAPPDATA\Programs\texe"
)

$ErrorActionPreference = "Stop"
if (-not (Test-Path -LiteralPath $From -PathType Leaf)) {
    throw "Archive not found: $From"
}
if ([string]::IsNullOrWhiteSpace($Prefix)) {
    throw "Refusing unsafe install prefix: $Prefix"
}
$Prefix = [System.IO.Path]::GetFullPath($Prefix)
if ($Prefix -eq [System.IO.Path]::GetPathRoot($Prefix)) {
    throw "Refusing unsafe install prefix: $Prefix"
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("texe-install-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $work | Out-Null
try {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead((Resolve-Path -LiteralPath $From))
    try {
        $root = [System.IO.Path]::GetFullPath($work) + [System.IO.Path]::DirectorySeparatorChar
        foreach ($entry in $zip.Entries) {
            $relative = $entry.FullName.Replace('/', [System.IO.Path]::DirectorySeparatorChar)
            $destination = [System.IO.Path]::GetFullPath((Join-Path $work $relative))
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
    Expand-Archive -LiteralPath $From -DestinationPath $work
    $bundle = Join-Path $work "texe-x86_64-windows"
    $expected = @{}
    Get-Content (Join-Path $bundle "SHA256SUMS") | ForEach-Object {
        if ($_ -match '^([0-9a-f]{64})\s+\*?(.+)$') {
            $expected[$Matches[2].Replace('/', '\')] = $Matches[1]
        }
    }
    foreach ($relative in @("bin\texe.exe", "bin\pqty.exe", "bin\pqty-fls.exe")) {
        $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $bundle $relative)).Hash.ToLowerInvariant()
        if ($actual -ne $expected[$relative]) {
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
