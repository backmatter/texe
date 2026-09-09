#!/bin/bash
set -euo pipefail
: "${TEXE_PREVIEW_VNC_PASSWORD:?missing VNC password}"
# These changes apply only to the disposable native testing VM.
# Grant only Apple's remote-management agents the required capture/input access.
# Do not disable SIP or weaken the system-wide privacy policy.
sudo python3 scripts/preview/mac_permissions.py
# Create a dedicated GUI test account; the runner's own login is not changed.
sudo sysadminctl -addUser texepreview -fullName "texe Preview" -password "$TEXE_PREVIEW_PASSWORD"
sudo mkdir -p /Users/texepreview/Library/LaunchAgents
sudo tee /Users/texepreview/Library/LaunchAgents/org.backmatter.texe-preview.plist >/dev/null <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>org.backmatter.texe-preview</string>
<key>ProgramArguments</key><array><string>/usr/bin/open</string><string>/Applications/texe.app</string></array>
<key>RunAtLoad</key><true/>
</dict></plist>
PLIST
sudo chown -R texepreview:staff /Users/texepreview/Library/LaunchAgents
# Allow choosing the dedicated test account from the native login screen.
sudo defaults write /Library/Preferences/.GlobalPreferences MultipleSessionEnabled -bool true
sudo defaults write /Library/Preferences/com.apple.loginwindow SHOWFULLNAME -bool true
sudo pmset displaysleep 0
caffeinate -u -t 6000 &
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
