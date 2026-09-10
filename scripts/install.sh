#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: install.sh [--from <archive>] [--prefix <directory>]

Downloads, verifies, and installs the latest texe command suite for this
computer. With --from, installs a previously downloaded archive instead.
The default prefix is ~/.local and administrator access is not required.
EOF
}

fail() {
  echo "install.sh: $1" >&2
  exit "${2:-1}"
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

[[ -n "$prefix" && "$prefix" != "/" ]] || fail "refusing unsafe install prefix: ${prefix:-<empty>}" 2

system="$(uname -s)"
machine="$(uname -m)"
translated=0
if [[ "$system:$machine" == "Darwin:x86_64" ]] \
    && [[ "$(sysctl -in sysctl.proc_translated 2>/dev/null || true)" == "1" ]]; then
  translated=1
fi
case "$system:$machine" in
  Linux:x86_64)
    bundle_name="texe-x86_64-linux"
    profile="${HOME:-}/.profile"
    ;;
  Darwin:arm64)
    bundle_name="texe-aarch64-macos"
    profile="${HOME:-}/.zprofile"
    ;;
  Darwin:x86_64)
    [[ "$translated" == "1" ]] || fail "texe supports Apple Silicon Macs, not Intel Macs" 2
    bundle_name="texe-aarch64-macos"
    profile="${HOME:-}/.zprofile"
    ;;
  *)
    fail "unsupported computer: $system $machine. texe supports Linux x86-64, Windows x86-64, and Apple Silicon macOS" 2
    ;;
esac
archive_name="$bundle_name.tar.gz"

if command -v sha256sum >/dev/null 2>&1; then
  sha256_of() { sha256sum "$1" | awk '{print $1}'; }
  sha256_verify_listed() { sha256sum -c SHA256SUMS; }
elif command -v shasum >/dev/null 2>&1; then
  sha256_of() { shasum -a 256 "$1" | awk '{print $1}'; }
  sha256_verify_listed() { shasum -a 256 -c SHA256SUMS; }
else
  fail "sha256sum or shasum is required to verify the download"
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/texe-install.XXXXXX")"
cleanup() {
  case "$work" in
    "${TMPDIR:-/tmp}"/texe-install.*) rm -rf -- "$work" ;;
    *) echo "install.sh: refusing to clean unexpected path: $work" >&2 ;;
  esac
}
trap cleanup EXIT

if [[ -z "$archive" ]]; then
  command -v curl >/dev/null 2>&1 || fail "curl is required to download texe"
  release_base="${TEXE_RELEASE_BASE_URL:-https://github.com/backmatter/texe/releases/latest/download}"
  case "$release_base" in
    https://*) ;;
    http://*)
      [[ "${TEXE_INSTALL_TEST_ALLOW_HTTP:-}" == "1" ]] || fail "refusing a non-HTTPS release URL" 2
      ;;
    *) fail "release URL must use HTTPS" 2 ;;
  esac
  download_release_file() {
    local url="$1"
    local destination="$2"
    if [[ "$url" == https://* ]]; then
      curl --proto "=https" --tlsv1.2 --fail --location --show-error --silent \
        "$url" --output "$destination"
    else
      curl --fail --location --show-error --silent \
        "$url" --output "$destination"
    fi
  }

  archive="$work/$archive_name"
  checksums="$work/SHA256SUMS"
  echo "downloading the latest texe command suite for $system $machine"
  download_release_file "$release_base/$archive_name" "$archive"
  download_release_file "$release_base/SHA256SUMS" "$checksums"

  expected="$(
    awk -v name="$archive_name" '$2 == name || $2 == "*" name { print $1; exit }' "$checksums" |
      tr 'A-F' 'a-f'
  )"
  case "$expected" in
    *[!0-9a-f]*|"") fail "release checksum is invalid" ;;
  esac
  [[ "${#expected}" -eq 64 ]] || fail "release checksum is invalid"
  [[ "$(sha256_of "$archive")" == "$expected" ]] \
    || fail "downloaded archive failed checksum verification"
elif [[ ! -f "$archive" ]]; then
  fail "archive not found: $archive" 2
fi

while IFS= read -r member; do
  case "$member" in
    /*|../*|*/../*|*/..) fail "archive contains unsafe path: $member" ;;
  esac
done < <(tar -tzf "$archive")
while IFS= read -r listing; do
  case "${listing:0:1}" in
    -|d) ;;
    *) fail "archive contains a link or unsupported entry: $listing" ;;
  esac
done < <(tar -tvzf "$archive")

tar --no-same-owner --no-same-permissions -C "$work" -xzf "$archive"
bundle="$work/$bundle_name"
[[ -d "$bundle" ]] || fail "archive does not contain the expected command suite"
(cd "$bundle" && sha256_verify_listed)

mkdir -p "$prefix/bin"
for binary in texe pqty pqty-fls; do
  [[ -f "$bundle/bin/$binary" ]] || fail "archive is missing $binary"
  install -m 755 "$bundle/bin/$binary" "$prefix/bin/$binary"
done

echo "installed texe, pqty, and pqty-fls to $prefix/bin"
case ":${PATH:-}:" in
  *":$prefix/bin:"*) echo "run: texe" ;;
  *)
    if [[ -n "${HOME:-}" && "$prefix" == "$HOME/.local" ]]; then
      marker="# >>> texe PATH"
      if ! grep -Fq "$marker" "$profile" 2>/dev/null; then
        {
          echo
          echo "$marker"
          echo 'export PATH="$HOME/.local/bin:$PATH"'
          echo "# <<< texe PATH"
        } >> "$profile"
      fi
      echo "open a new terminal, then run: texe"
    else
      echo "add $prefix/bin to PATH, then run: texe"
    fi
    ;;
esac
