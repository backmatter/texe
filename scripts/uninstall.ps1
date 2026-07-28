param(
    [string]$Prefix = "$env:LOCALAPPDATA\Programs\texe"
)
$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($Prefix)) {
    throw "Refusing unsafe uninstall prefix: $Prefix"
}
$Prefix = [System.IO.Path]::GetFullPath($Prefix)
if ($Prefix -eq [System.IO.Path]::GetPathRoot($Prefix)) {
    throw "Refusing unsafe uninstall prefix: $Prefix"
}
$bin = Join-Path $Prefix "bin"
foreach ($binary in @("texe.exe", "pqty.exe", "pqty-fls.exe")) {
    $path = Join-Path $bin $binary
    if (Test-Path -LiteralPath $path -PathType Leaf) { Remove-Item -LiteralPath $path -Force }
}
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$remaining = @($userPath -split ';' | Where-Object { $_ -and $_ -ne $bin })
[Environment]::SetEnvironmentVariable("Path", ($remaining -join ';'), "User")
Write-Host "Removed texe application files from $bin"
Write-Host "Managed runtimes and caches were kept"
