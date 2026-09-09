param(
    [Parameter(Mandatory=$true)][string]$SuiteBin,
    [Parameter(Mandatory=$true)][string]$OutputDir,
    [Parameter(Mandatory=$true)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version,
    [switch]$TestInstall
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$SuiteBin = (Resolve-Path -LiteralPath $SuiteBin).Path
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$OutputDir = (Resolve-Path -LiteralPath $OutputDir).Path
$stage = Join-Path ([IO.Path]::GetTempPath()) ("texe-desktop-" + [guid]::NewGuid())
try {
    New-Item -ItemType Directory -Path (Join-Path $stage 'bin') -Force | Out-Null
    foreach ($tool in 'texe', 'pqty', 'pqty-fls') {
        Copy-Item -LiteralPath (Join-Path $SuiteBin "$tool.exe") -Destination (Join-Path $stage 'bin')
    }
    Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination $stage
    Copy-Item -LiteralPath (Join-Path $repo 'assets/pdfjs/LICENSE') -Destination (Join-Path $stage 'PDFJS-LICENSE')
    $compiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
    $app = Join-Path $stage 'texe-desktop.exe'
    $source = (Resolve-Path -LiteralPath (Join-Path $repo 'desktop/windows/Welcome.cs')).Path
    $manifest = (Resolve-Path -LiteralPath (Join-Path $repo 'desktop/windows/app.manifest')).Path
    & $compiler /nologo /codepage:65001 /target:winexe /platform:x64 /optimize+ "/out:$app" `
        "/win32manifest:$manifest" `
        /reference:System.Windows.Forms.dll /reference:System.Drawing.dll /reference:System.Core.dll `
        $source
    if ($LASTEXITCODE -ne 0) { throw 'Native welcome app compilation failed' }
    foreach ($check in '--test-quoting', '--smoke-test') {
        $process = Start-Process -FilePath $app -ArgumentList $check -Wait -PassThru
        if ($process.ExitCode -ne 0) { throw "Welcome app $check failed" }
    }
    # Optional certificate in the current user's certificate store. Never export a private key.
    if ($env:TEXE_WINDOWS_CERT_THUMBPRINT) {
        if (-not $env:TEXE_WINDOWS_TIMESTAMP_URL) { throw 'Set TEXE_WINDOWS_TIMESTAMP_URL for signed releases' }
        foreach ($binary in @($app) + @(Get-ChildItem (Join-Path $stage 'bin/*.exe') | ForEach-Object FullName)) {
            & signtool sign /sha1 $env:TEXE_WINDOWS_CERT_THUMBPRINT /fd SHA256 `
                /tr $env:TEXE_WINDOWS_TIMESTAMP_URL /td SHA256 $binary
            if ($LASTEXITCODE -ne 0) { throw "Signing failed: $binary" }
        }
    }
    $iscc = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source
    if (-not $iscc) { $iscc = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe' }
    $arguments = @("/DStage=$stage", "/DOutputDir=$OutputDir", "/DVersion=$Version")
    if ($env:TEXE_WINDOWS_CERT_THUMBPRINT) {
        $sign = 'signtool sign /sha1 ' + $env:TEXE_WINDOWS_CERT_THUMBPRINT +
            ' /fd SHA256 /tr ' + $env:TEXE_WINDOWS_TIMESTAMP_URL + ' /td SHA256 $f'
        $arguments += @('/DSignCommand=1', "/Srelease=$sign")
    }
    & $iscc @arguments (Join-Path $repo 'packaging/windows/texe.iss')
    if ($LASTEXITCODE -ne 0) { throw 'Windows installer compilation failed' }
    $installer = Join-Path $OutputDir 'texe-x86_64-windows-setup.exe'
    if ($env:TEXE_WINDOWS_CERT_THUMBPRINT) {
        & signtool verify /pa $installer
        if ($LASTEXITCODE -ne 0) { throw 'Installer signature verification failed' }
    }
    # Installation testing is opt-in and refuses to replace an existing install.
    if ($TestInstall) {
        $uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{5D637AAF-6BB6-4CAE-8BAC-9736CC0EA6C8}_is1'
        if (Test-Path $uninstallKey) { throw 'Refusing to test over an existing texe desktop installation' }
        $installed = Join-Path $stage 'installed'
        $process = Start-Process -FilePath $installer -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/NOICONS', "/DIR=`"$installed`"") -Wait -PassThru
        if ($process.ExitCode -ne 0) { throw 'Installer smoke test failed' }
        try {
            $process = Start-Process -FilePath (Join-Path $installed 'texe-desktop.exe') -ArgumentList '--smoke-test' -Wait -PassThru
            if ($process.ExitCode -ne 0) { throw 'Installed welcome app smoke test failed' }
        } finally {
            $process = Start-Process -FilePath (Join-Path $installed 'unins000.exe') -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait -PassThru
            if ($process.ExitCode -ne 0) { throw 'Uninstaller smoke test failed' }
        }
        if (Test-Path -LiteralPath (Join-Path $installed 'texe-desktop.exe')) { throw 'Uninstaller left the app behind' }
    }
    Write-Output "wrote $installer"
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
