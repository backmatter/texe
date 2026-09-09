#!/bin/bash
set -euo pipefail
: "${TEXE_PREVIEW_VNC_PASSWORD:?missing VNC password}"
# These changes apply only to the disposable native testing VM.
agent=/System/Library/CoreServices/RemoteManagement/ARDAgent.app/Contents/Resources/kickstart
sudo "$agent" -activate -configure -access -on -users "$(whoami)" -privs -all
sudo "$agent" -configure -clientopts -setvnclegacy -vnclegacy yes
sudo "$agent" -configure -clientopts -setvncpw -vncpw "$TEXE_PREVIEW_VNC_PASSWORD"
sudo launchctl enable system/com.apple.screensharing
sudo launchctl kickstart -k system/com.apple.screensharing || \
  sudo launchctl load -w /System/Library/LaunchDaemons/com.apple.screensharing.plist
# Keep raw VNC off network interfaces; only the authenticated gateway is tunneled.
printf 'block in quick on ! lo0 proto tcp from any to any port 5900\n' | \
  sudo pfctl -a com.apple/texe-preview -f -
sudo pfctl -E
open /Applications/texe.app
python3 -m venv "$RUNNER_TEMP/texe-preview-venv"
"$RUNNER_TEMP/texe-preview-venv/bin/python" -m pip install --disable-pip-version-check websockify==0.13.0
"$RUNNER_TEMP/texe-preview-venv/bin/python" scripts/preview/serve.py --background
