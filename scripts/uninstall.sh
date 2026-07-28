#!/usr/bin/env bash
set -euo pipefail

prefix="${HOME:-}/.local"
while (($#)); do
  case "$1" in
    --prefix)
      [[ $# -ge 2 ]] || {
        echo "usage: uninstall.sh [--prefix <directory>]" >&2
        exit 2
      }
      prefix="$2"
      shift 2
      ;;
    -h|--help)
      echo "usage: uninstall.sh [--prefix <directory>]" >&2
      exit 0
      ;;
    *) echo "uninstall.sh: unknown argument: $1" >&2; exit 2 ;;
  esac
done
[[ -n "$prefix" && "$prefix" != "/" ]] || {
  echo "uninstall.sh: refusing unsafe prefix" >&2
  exit 2
}
for binary in texe pqty pqty-fls; do
  path="$prefix/bin/$binary"
  [[ -f "$path" ]] && rm -- "$path"
done
if [[ "$prefix" == "${HOME:?}/.local" ]]; then
  case "$(uname -s)" in
    Darwin) profile="$HOME/.zprofile" ;;
    *) profile="$HOME/.profile" ;;
  esac
  if [[ -f "$profile" ]] && grep -Fq "# >>> texe PATH" "$profile"; then
    temporary="$(mktemp "${TMPDIR:-/tmp}/texe-profile.XXXXXX")"
    awk '
      $0 == "# >>> texe PATH" { owned = 1; next }
      $0 == "# <<< texe PATH" { owned = 0; next }
      !owned { print }
    ' "$profile" > "$temporary"
    mv -- "$temporary" "$profile"
  fi
fi
echo "removed texe application files from $prefix/bin"
echo "managed runtimes and caches were kept"
