"""Provision narrowly scoped native screen-sharing consent on a disposable CI Mac."""
import os
import sqlite3
import time

if os.environ.get('GITHUB_ACTIONS') != 'true' and os.environ.get('SUDO_USER') != 'runner':
    raise SystemExit('This helper is only for the disposable GitHub test runner')
path = '/Library/Application Support/com.apple.TCC/TCC.db'
with sqlite3.connect(path) as database:
    columns = {row[1] for row in database.execute('PRAGMA table_info(access)')}
    if 'auth_value' not in columns:
        raise SystemExit('Unsupported privacy database; no permissions were changed')
    for client in ['com.apple.screensharing.agent', 'com.apple.RemoteDesktop.agent', 'com.apple.screensharingd']:
        for service in ['kTCCServiceScreenCapture', 'kTCCServiceAccessibility', 'kTCCServicePostEvent']:
            row = dict(service=service, client=client, client_type=0, auth_value=2,
                       auth_reason=4, auth_version=1, indirect_object_identifier='UNUSED',
                       last_modified=int(time.time()), flags=0)
            values = {key: value for key, value in row.items() if key in columns}
            names = ','.join(values)
            placeholders = ','.join('?' for _ in values)
            database.execute('INSERT OR REPLACE INTO access (' + names + ') VALUES (' + placeholders + ')', list(values.values()))
print('Configured capture and input permissions for native screen-sharing agents')
