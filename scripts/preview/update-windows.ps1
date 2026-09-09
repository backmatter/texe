param([string]$Revision = 'codex/native-welcome')
# Refresh only the welcome executable on a disposable interactive test VM.
$ErrorActionPreference = 'Stop'
if (-not (Test-Path 'D:\a\_temp\texe-browser-preview') -and -not (Test-Path 'C:\a\_temp\texe-browser-preview')) {
    throw 'This helper is only for the disposable preview runner'
}
$base = "https://raw.githubusercontent.com/backmatter/texe/$Revision/"
$stage = Join-Path $env:TEMP ('texe-refresh-' + [guid]::NewGuid())
New-Item -ItemType Directory $stage | Out-Null
try {
    foreach ($item in @('desktop/windows/Welcome.cs', 'desktop/windows/app.manifest',
                        'desktop/brand/texe/favicon.ico', 'desktop/brand/texe/logo-wordmark-dark.png')) {
        Invoke-WebRequest ($base + $item) -OutFile (Join-Path $stage (Split-Path $item -Leaf))
    }
    $compiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
    $app = Join-Path $stage 'texe-desktop.exe'
    $manifest = Join-Path $stage 'app.manifest'
    $icon = Join-Path $stage 'favicon.ico'
    $wordmark = Join-Path $stage 'logo-wordmark-dark.png'
    & $compiler /nologo /warnaserror /codepage:65001 /target:winexe /platform:x64 "/out:$app" `
        "/win32manifest:$manifest" "/win32icon:$icon" "/resource:$icon,texe.icon" "/resource:$wordmark,texe.wordmark" `
        /reference:System.Windows.Forms.dll /reference:System.Drawing.dll /reference:System.Core.dll `
        (Join-Path $stage 'Welcome.cs')
    if ($LASTEXITCODE -ne 0) { throw 'Native compilation failed' }
    $check = Start-Process $app -ArgumentList '--test-quoting' -Wait -PassThru
    if ($check.ExitCode -ne 0) { throw 'Native dialog or argument check failed' }
    foreach ($process in @(Get-Process texe-desktop -ErrorAction SilentlyContinue)) {
        if (-not $process.CloseMainWindow() -or -not $process.WaitForExit(3000)) {
            throw 'The app is busy. Finish its current setup before refreshing.'
        }
    }
    $installed = Join-Path $env:LOCALAPPDATA 'Programs/texe-desktop/texe-desktop.exe'
    Copy-Item $app $installed -Force
    Start-Process $installed
} finally {
    Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
}
