#!/bin/bash
# Build on an Apple Silicon Mac with Xcode command-line tools.
set -euo pipefail
if [[ $# != 3 ]]; then
  echo "usage: $0 SUITE_BIN OUTPUT_DMG VERSION" >&2
  exit 2
fi
[[ "$(uname -s):$(uname -m)" == Darwin:arm64 ]]
suite_bin="$(cd "$1" && pwd)"
output="$2"
version="$3"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
repo="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$(dirname "$output")"
output="$(cd "$(dirname "$output")" && pwd)/$(basename "$output")"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
app="$scratch/image/texe.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
for tool in texe pqty pqty-fls; do
  test -x "$suite_bin/$tool"
  cp "$suite_bin/$tool" "$app/Contents/MacOS/$tool"
done
xcrun swiftc -swift-version 5 -O -target arm64-apple-macos12.0 \
  -framework AppKit "$repo/desktop/macos/main.swift" -o "$app/Contents/MacOS/texe-desktop"
sed "s/@VERSION@/$version/g" "$repo/packaging/macos/Info.plist.in" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
cp "$repo/LICENSE" "$app/Contents/Resources/LICENSE"
cp "$repo/assets/pdfjs/LICENSE" "$app/Contents/Resources/PDFJS-LICENSE"
# Developer ID identity and notary credentials are supplied by the release host.
# Ad-hoc signing supports local testing, but does not replace notarization.
identity="${TEXE_MACOS_SIGN_IDENTITY:--}"
sign_options=(--force --options runtime --sign "$identity")
if [[ "$identity" != - ]]; then sign_options+=(--timestamp); fi
for tool in texe pqty pqty-fls texe-desktop; do
  codesign "${sign_options[@]}" "$app/Contents/MacOS/$tool"
done
codesign "${sign_options[@]}" "$app"
codesign --verify --deep --strict "$app"
"$app/Contents/MacOS/texe-desktop" --smoke-test
if [[ -n "${TEXE_MACOS_NOTARY_PROFILE:-}" ]]; then
  [[ "$identity" != - ]]
  ditto -c -k --keepParent "$app" "$scratch/notarize.zip"
  xcrun notarytool submit "$scratch/notarize.zip" --keychain-profile "$TEXE_MACOS_NOTARY_PROFILE" --wait
  xcrun stapler staple "$app"
  spctl --assess --type execute "$app"
fi
ln -s /Applications "$scratch/image/Applications"
hdiutil create -volname "texe" -srcfolder "$scratch/image" -format UDZO "$output"
if [[ "$identity" != - ]]; then
  codesign --sign "$identity" --timestamp "$output"
fi
if [[ -n "${TEXE_MACOS_NOTARY_PROFILE:-}" ]]; then
  xcrun notarytool submit "$output" --keychain-profile "$TEXE_MACOS_NOTARY_PROFILE" --wait
  xcrun stapler staple "$output"
fi
hdiutil verify "$output"
echo "wrote $output"
