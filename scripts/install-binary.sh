#!/bin/bash
set -euo pipefail

# This script ships next to a compiled mtr binary. Rust is not needed here.
case "${1:-}" in
  ""|--force) ;;
  *) echo "Usage: bash install.sh [--force]" >&2; exit 2 ;;
esac
package_dir="$(cd -- "$(dirname -- "$0")" && pwd)"
install_dir="${MTR_INSTALL_DIR:-$HOME/.local/bin}"
if [[ ! -f "$package_dir/mtr" ]]; then
  echo "Missing mtr binary beside this script. Extract the complete release archive first." >&2
  exit 1
fi
if [[ -e "$install_dir/mtr" || -L "$install_dir/mtr" ]]; then
  if [[ "${1:-}" != --force ]]; then
    echo "$install_dir/mtr already exists. Use --force only if you want to replace it." >&2
    exit 1
  fi
fi
mkdir -p "$install_dir"
install -m 755 "$package_dir/mtr" "$install_dir/mtr"
echo "Installed: $install_dir/mtr"
case ":$PATH:" in
  *":$install_dir:"*) echo "Run: mtr" ;;
  *)
    printf 'Add this line to your ~/.zshrc, then open a new Terminal:\n'
    printf 'export PATH=%q:"$PATH"\n' "$install_dir"
    ;;
esac
echo "If another program is also named mtr, use 'type -a mtr' to check which one runs."
