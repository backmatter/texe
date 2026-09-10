"""Wait for the protected test session, or hold its disposable runner until expiry."""
import datetime
import json
import os
from pathlib import Path
import sys
import time
import urllib.error
import urllib.request

root = Path(os.environ['RUNNER_TEMP']) / 'texe-browser-preview'
metadata = root / 'session.json'
if sys.argv[1] == 'ready':
    for attempt in range(180):
        if metadata.exists():
            data = json.loads(metadata.read_text())
            print('Interactive preview: ' + data['url'])
            print('Expires: ' + data['expires'])
            break
        time.sleep(1)
    else:
        print((root / 'serve.log').read_text())
        raise SystemExit('The native browser session did not become ready')
else:
    data = json.loads(metadata.read_text())
    expires = datetime.datetime.fromisoformat(data['expires'])
    while datetime.datetime.now(datetime.timezone.utc) < expires:
        try:
            urllib.request.urlopen('http://127.0.0.1:6080/vnc.html', timeout=5)
        except urllib.error.HTTPError as error:
            if error.code != 401:
                raise
        except OSError:
            print('Preview gateway closed')
            break
        else:
            raise SystemExit('Preview gateway lost authentication')
        time.sleep(15)
    print('Interactive testing session ended')
