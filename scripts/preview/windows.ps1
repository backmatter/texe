$ErrorActionPreference = 'Stop'
if (-not $env:TEXE_PREVIEW_VNC_PASSWORD) { throw 'Missing VNC password' }
$msi = Join-Path $env:RUNNER_TEMP 'tightvnc.msi'
Invoke-WebRequest 'https://www.tightvnc.com/download/2.8.88/tightvnc-2.8.88-gpl-setup-64bit.msi' -OutFile $msi
$arguments = @('/i', "`"$msi`"", '/quiet', '/norestart', 'ADDLOCAL=Server',
    'SERVER_REGISTER_AS_SERVICE=1', 'SERVER_ADD_FIREWALL_EXCEPTION=0',
    'SET_USEVNCAUTHENTICATION=1', 'VALUE_OF_USEVNCAUTHENTICATION=1',
    'SET_PASSWORD=1', "VALUE_OF_PASSWORD=$env:TEXE_PREVIEW_VNC_PASSWORD",
    'SET_USECONTROLAUTHENTICATION=1', 'VALUE_OF_USECONTROLAUTHENTICATION=1',
    'SET_CONTROLPASSWORD=1', "VALUE_OF_CONTROLPASSWORD=$env:TEXE_PREVIEW_VNC_PASSWORD")
$process = Start-Process msiexec.exe -ArgumentList $arguments -Wait -PassThru
if ($process.ExitCode -notin 0,3010) { throw 'Screen-sharing server installation failed' }
Stop-Service tvnserver -ErrorAction SilentlyContinue
$key = 'HKLM:\SOFTWARE\TightVNC\Server'
Set-ItemProperty $key -Name LoopbackOnly -Value 1 -Type DWord
Set-ItemProperty $key -Name AllowLoopback -Value 1 -Type DWord
Start-Service tvnserver
Start-Process (Join-Path $env:LOCALAPPDATA 'Programs/texe-desktop/texe-desktop.exe')
python -m venv "$env:RUNNER_TEMP/texe-preview-venv"
$python = Join-Path $env:RUNNER_TEMP 'texe-preview-venv/Scripts/python.exe'
& $python -m pip install --disable-pip-version-check websockify==0.13.0
if ($LASTEXITCODE -ne 0) { throw 'Browser gateway installation failed' }
& $python scripts/preview/serve.py --background
if ($LASTEXITCODE -ne 0) { throw 'Interactive preview stopped' }
