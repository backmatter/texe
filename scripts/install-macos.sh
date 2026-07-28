#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: install-macos.sh [--from <texe-aarch64-macos.tar.gz>] [--prefix <directory>]

With no --from argument, download the latest verified suite from GitHub.
The default installation prefix is ~/.local and administrator access is not
required.
EOF
}

archive=""
prefix="${HOME:-}/.local"
while (($#)); do
  case "$1" in
    --from)
      [[ $# -ge 2 ]] || { usage; exit 2; }
      archive="$2"
      shift 2
      ;;
    --prefix)
      [[ $# -ge 2 ]] || { usage; exit 2; }
      prefix="$2"
      shift 2
      ;;
    -h|--help) usage; exit 0 ;;
    *) usage; exit 2 ;;
  esac
done

[[ -n "$prefix" && "$prefix" != "/" ]] || {
  echo "install-macos: refusing unsafe prefix" >&2
  exit 2
}
system="$(uname -s)"
machine="$(uname -m)"
translated=0
if [[ "$system:$machine" == "Darwin:x86_64" ]] \
    && [[ "$(sysctl -in sysctl.proc_translated 2>/dev/null || true)" == "1" ]]; then
  translated=1
fi
if [[ "$system:$machine" != "Darwin:arm64" && "$translated" != "1" ]]; then
  echo "install-macos: this installer supports macOS on Apple Silicon" >&2
  exit 2
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/texe-install.XXXXXX")"
cleanup() {
  case "$work" in
    "${TMPDIR:-/tmp}"/texe-install.*) rm -rf -- "$work" ;;
    *) echo "install-macos: refusing unexpected cleanup path" >&2 ;;
  esac
}
trap cleanup EXIT

if [[ -z "$archive" ]]; then
  command -v curl >/dev/null 2>&1 || {
    echo "install-macos: curl is required to download texe" >&2
    exit 1
  }
  release_base="${TEXE_RELEASE_BASE_URL:-https://github.com/backmatter/texe/releases/latest/download}"
  case "$release_base" in
    https://*) curl_security=(--proto "=https" --tlsv1.2) ;;
    http://*)
      [[ "${TEXE_INSTALL_TEST_ALLOW_HTTP:-}" == "1" ]] || {
        echo "install-macos: refusing a non-HTTPS release URL" >&2
        exit 2
      }
      curl_security=()
      ;;
    *)
      echo "install-macos: release URL must use HTTPS" >&2
      exit 2
      ;;
  esac
  archive="$work/texe-aarch64-macos.tar.gz"
  checksums="$work/SHA256SUMS"
  echo "downloading the latest texe command suite"
  curl "${curl_security[@]}" --fail --location --show-error --silent \
    "$release_base/texe-aarch64-macos.tar.gz" --output "$archive"
  curl "${curl_security[@]}" --fail --location --show-error --silent \
    "$release_base/SHA256SUMS" --output "$checksums"

  expected="$(
    awk '$2 == "texe-aarch64-macos.tar.gz" { print $1; exit }' "$checksums" |
      tr 'A-F' 'a-f'
  )"
  case "$expected" in
    *[!0-9A-Fa-f]*|"")
      echo "install-macos: release checksum is invalid" >&2
      exit 1
      ;;
  esac
  [[ "${#expected}" -eq 64 ]] || {
    echo "install-macos: release checksum is invalid" >&2
    exit 1
  }
  actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
  [[ "$actual" == "$expected" ]] || {
    echo "install-macos: downloaded archive failed checksum verification" >&2
    exit 1
  }
elif [[ ! -f "$archive" ]]; then
  echo "install-macos: archive not found: $archive" >&2
  exit 2
fi

while IFS= read -r member; do
  case "$member" in
    /*|../*|*/../*|*/..)
      echo "install-macos: archive contains unsafe path: $member" >&2
      exit 1
      ;;
  esac
done < <(tar -tzf "$archive")
while IFS= read -r listing; do
  case "${listing:0:1}" in
    -|d) ;;
    *)
      echo "install-macos: archive contains a link or unsupported entry: $listing" >&2
      exit 1
      ;;
  esac
done < <(tar -tvzf "$archive")

tar --no-same-owner --no-same-permissions -C "$work" -xzf "$archive"
bundle="$work/texe-aarch64-macos"
[[ -d "$bundle" ]] || {
  echo "install-macos: archive does not contain the expected command suite" >&2
  exit 1
}
(cd "$bundle" && shasum -a 256 -c SHA256SUMS)

mkdir -p "$prefix/bin"
for binary in texe pqty pqty-fls; do
  [[ -f "$bundle/bin/$binary" ]] || {
    echo "install-macos: archive is missing $binary" >&2
    exit 1
  }
  install -m 755 "$bundle/bin/$binary" "$prefix/bin/$binary"
done

echo "installed texe, pqty, and pqty-fls to $prefix/bin"
case ":${PATH:-}:" in
  *":$prefix/bin:"*) echo "run: texe" ;;
  *)
    if [[ "$prefix" == "${HOME:?}/.local" ]]; then
      profile="$HOME/.zprofile"
      marker="# >>> texe PATH"
      if ! grep -Fq "$marker" "$profile" 2>/dev/null; then
        {
          echo
          echo "$marker"
          echo 'export PATH="$HOME/.local/bin:$PATH"'
          echo "# <<< texe PATH"
        } >> "$profile"
      fi
      echo "open a new Terminal window, then run: texe"
    else
      echo "add $prefix/bin to PATH, then run: texe"
    fi
    ;;
esac
