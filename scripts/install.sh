#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: install.sh --from <texe-x86_64-linux.tar.gz> [--prefix <directory>]" >&2
}

archive=""
prefix="${HOME:-}/.local"
while (($#)); do
  case "$1" in
    --from)
      [[ $# -ge 2 ]] || {
        usage
        exit 2
      }
      archive="$2"
      shift 2
      ;;
    --prefix)
      [[ $# -ge 2 ]] || {
        usage
        exit 2
      }
      prefix="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

if [[ -z "$archive" || ! -f "$archive" ]]; then
  usage
  exit 2
fi
if [[ -z "$prefix" || "$prefix" == "/" ]]; then
  echo "install.sh: refusing unsafe install prefix: ${prefix:-<empty>}" >&2
  exit 2
fi

while IFS= read -r member; do
  case "$member" in
    /*|../*|*/../*|*/..)
      echo "install.sh: archive contains unsafe path: $member" >&2
      exit 1
      ;;
  esac
done < <(tar -tzf "$archive")
while IFS= read -r listing; do
  case "${listing:0:1}" in
    -|d) ;;
    *)
      echo "install.sh: archive contains a link or unsupported entry: $listing" >&2
      exit 1
      ;;
  esac
done < <(tar -tvzf "$archive")

work="$(mktemp -d "${TMPDIR:-/tmp}/texe-install.XXXXXX")"
cleanup() {
  case "$work" in
    "${TMPDIR:-/tmp}"/texe-install.*) rm -rf -- "$work" ;;
    *) echo "install.sh: refusing to clean unexpected path: $work" >&2 ;;
  esac
}
trap cleanup EXIT

tar --no-same-owner --no-same-permissions -C "$work" -xzf "$archive"
bundle="$work/texe-x86_64-linux"
(
  cd "$bundle"
  sha256sum -c SHA256SUMS
)
mkdir -p "$prefix/bin"
for binary in texe pqty pqty-fls; do
  install -m 755 "$bundle/bin/$binary" "$prefix/bin/$binary"
done

echo "installed texe, pqty, and pqty-fls to $prefix/bin"
case ":${PATH:-}:" in
  *":$prefix/bin:"*) ;;
  *)
    if [[ "$prefix" == "${HOME:?}/.local" ]]; then
      profile="$HOME/.profile"
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
      echo "add $prefix/bin to PATH"
    fi
    ;;
esac
