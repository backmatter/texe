"""Write a restricted launchd job for the disposable Mac preview gateway."""
import os
from pathlib import Path
import plistlib

root = Path(os.environ['RUNNER_TEMP'])
preview = root / 'texe-browser-preview'
preview.mkdir(exist_ok=True)
job = {
    'Label': 'org.backmatter.texe-preview',
    'UserName': os.environ['TEXE_PREVIEW_USER'],
    'ProgramArguments': [str(root / 'texe-preview-venv/bin/python'),
                         str(Path(__file__).with_name('serve.py').resolve())],
    'EnvironmentVariables': {name: os.environ[name] for name in
                             ('RUNNER_TEMP', 'TEXE_PREVIEW_PASSWORD', 'TEXE_PREVIEW_VNC_PASSWORD', 'PATH')},
    'RunAtLoad': True,
    'StandardOutPath': str(preview / 'serve.log'),
    'StandardErrorPath': str(preview / 'serve.log'),
}
path = root / 'org.backmatter.texe-preview.plist'
with path.open('wb') as output:
    plistlib.dump(job, output)
path.chmod(0o600)
