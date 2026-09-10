"""Time-limited, authenticated browser access for testing texe on a CI runner."""
import base64
import datetime
import json
import os
from pathlib import Path
import platform
import re
import socket
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.request
import zipfile

root = Path(os.environ['RUNNER_TEMP']) / 'texe-browser-preview'
root.mkdir(exist_ok=True)
if '--background' in sys.argv:
    with (root / 'serve.log').open('w') as log:
        subprocess.Popen([sys.executable, str(Path(__file__).resolve())], stdout=log,
                         stderr=subprocess.STDOUT, start_new_session=True)
    raise SystemExit(0)
password = os.environ['TEXE_PREVIEW_PASSWORD']
if len(password) < 24:
    raise SystemExit('A strong preview password is required')

# Static noVNC client: never expose the workspace or any runner credentials.
archive = root / 'novnc.zip'
urllib.request.urlretrieve('https://github.com/novnc/noVNC/archive/refs/tags/v1.6.0.zip', archive)
with zipfile.ZipFile(archive) as source:
    source.extractall(root)
web = root / 'noVNC-1.6.0'
# Use the dedicated preview VNC password, not the Mac's account authentication.
rfb = web / 'core/rfb.js'
source = rfb.read_text()
needle = '_isSupportedSecurityType(type) {'
if source.count(needle) != 1:
    raise SystemExit('Unexpected noVNC authentication implementation')
rfb.write_text(source.replace(needle, needle + '\n        if (type !== 2) return false;'))
if platform.system() == 'Windows':
    tunnel = root / 'cloudflared.exe'
    urllib.request.urlretrieve('https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe', tunnel)
else:
    archive = root / 'cloudflared.tgz'
    urllib.request.urlretrieve('https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-arm64.tgz', archive)
    with tarfile.open(archive) as source:
        source.extractall(root, filter='data')
    tunnel = root / 'cloudflared'
    tunnel.chmod(0o755)

# Check that a VNC server is ready before publishing any link.
with socket.create_connection(('127.0.0.1', 5900), timeout=15) as connection:
    if not connection.recv(12).startswith(b'RFB '):
        raise SystemExit('The native screen-sharing server is not ready')

proxy_log = (root / 'proxy.log').open('w')
tunnel_log_path = root / 'tunnel.log'
tunnel_log = tunnel_log_path.open('w')
env = os.environ.copy()
env['PYTHONPATH'] = str(Path(__file__).resolve().parent)
proxy = subprocess.Popen([
    sys.executable, '-m', 'websockify', '--web', str(web), '--web-auth',
    '--auth-plugin', 'preview_auth.PreviewAuth', '--auth-source', 'texe:' + password,
    '127.0.0.1:6080', '127.0.0.1:5900',
], env=env, stdout=proxy_log, stderr=subprocess.STDOUT)
cloud = None
try:
    local = 'http://127.0.0.1:6080/vnc.html'
    for attempt in range(30):
        if proxy.poll() is not None:
            raise RuntimeError('The authenticated browser gateway stopped')
        try:
            urllib.request.urlopen(local, timeout=2)
        except urllib.error.HTTPError as error:
            if error.code == 401:
                break
            raise
        except OSError:
            time.sleep(1)
        else:
            raise RuntimeError('The browser gateway did not require authentication')
    else:
        raise RuntimeError('The browser gateway did not start')
    credentials = base64.b64encode(('texe:' + password).encode()).decode()
    request = urllib.request.Request(local, headers={'Authorization': 'Basic ' + credentials})
    with urllib.request.urlopen(request, timeout=5) as response:
        if response.status != 200:
            raise RuntimeError('Gateway login failed')
    cloud = subprocess.Popen([str(tunnel), 'tunnel', '--no-autoupdate', '--url', 'http://127.0.0.1:6080'],
                             stdout=tunnel_log, stderr=subprocess.STDOUT)
    for attempt in range(60):
        if cloud.poll() is not None:
            raise RuntimeError('The preview tunnel stopped')
        match = re.search(r'https://[a-z0-9-]+\.trycloudflare\.com', tunnel_log_path.read_text())
        if match:
            break
        time.sleep(1)
    else:
        raise RuntimeError('No preview URL was assigned')
    expires = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(minutes=90)
    url = match.group() + '/vnc.html?autoconnect=true&resize=scale'
    (root / 'session.json').write_text(json.dumps({'url': url, 'expires': expires.isoformat()}))
    print('PREVIEW_URL=' + url, flush=True)
    print('PREVIEW_EXPIRES=' + expires.isoformat(), flush=True)
    deadline = time.monotonic() + 90 * 60
    while time.monotonic() < deadline:
        if proxy.poll() is not None or cloud.poll() is not None:
            raise RuntimeError('Preview connection closed unexpectedly')
        time.sleep(15)
finally:
    for process in [cloud, proxy]:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
    proxy_log.close()
    tunnel_log.close()
