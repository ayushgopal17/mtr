#!/bin/bash
set -euo pipefail
export LC_ALL=C

# Native builds can be shared with Macs of the same CPU architecture.
# --universal combines native ARM64 and Intel builds into one executable.
case "${1:-}" in
  ""|--universal) ;;
  *) echo "Usage: bash scripts/package-macos.sh [--universal]" >&2; exit 2 ;;
esac
[[ "$(uname -s)" == Darwin ]] || { echo "Run this script on macOS." >&2; exit 1; }
command -v cargo >/dev/null || { echo "Install Rust from https://rustup.rs first." >&2; exit 1; }
project_dir="$(cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$project_dir"
export MACOSX_DEPLOYMENT_TARGET=12.0
export CARGO_TARGET_DIR="$project_dir/target"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
cargo test --locked
mkdir -p dist
stage="$(mktemp -d "$project_dir/dist/.package.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

if [[ "${1:-}" == --universal ]]; then
  rustup target add aarch64-apple-darwin x86_64-apple-darwin
  cargo build --release --locked --target aarch64-apple-darwin
  cargo build --release --locked --target x86_64-apple-darwin
  platform=macos-universal
  mkdir -p "$stage/mtr-$version-$platform"
  lipo -create target/aarch64-apple-darwin/release/mtr target/x86_64-apple-darwin/release/mtr \
    -output "$stage/mtr-$version-$platform/mtr"
else
  host="$(rustc -vV | sed -n 's/^host: //p')"
  case "$host" in
    aarch64-apple-darwin) platform=macos-arm64 ;;
    x86_64-apple-darwin) platform=macos-x86_64 ;;
    *) echo "Unsupported macOS host: $host" >&2; exit 1 ;;
  esac
  cargo build --release --locked --target "$host"
  mkdir -p "$stage/mtr-$version-$platform"
  cp "target/$host/release/mtr" "$stage/mtr-$version-$platform/mtr"
fi
bundle="mtr-$version-$platform"
codesign --force --sign - "$stage/$bundle/mtr"
codesign --verify "$stage/$bundle/mtr"
cp README.md LICENSE "$stage/$bundle/"
cp scripts/install-binary.sh "$stage/$bundle/install.sh"
chmod 755 "$stage/$bundle/mtr" "$stage/$bundle/install.sh"
archive="$bundle.tar.gz"
tar -czf "dist/$archive" -C "$stage" "$bundle"
(cd dist && shasum -a 256 "$archive" > "$archive.sha256")
printf '\nShare these files:\n  %s/dist/%s\n  %s/dist/%s.sha256\n' "$project_dir" "$archive" "$project_dir" "$archive"
echo "For macOS 12 or newer. Recipients do not need Rust."
echo "This is locally ad-hoc signed, not Apple Developer ID signed or notarized."
