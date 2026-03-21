#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
output_dir="${1:-dist}"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required but was not found in PATH." >&2
  exit 1
fi

if ! command -v cargo-deb >/dev/null 2>&1; then
  echo "cargo-deb is required. Install it with: cargo install cargo-deb" >&2
  exit 1
fi

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repo_root/Cargo.toml" | head -n 1)"
if [[ -z "$version" ]]; then
  echo "Could not determine workspace version from Cargo.toml." >&2
  exit 1
fi

if command -v dpkg >/dev/null 2>&1; then
  arch="$(dpkg --print-architecture)"
else
  case "$(uname -m)" in
    x86_64) arch="amd64" ;;
    aarch64) arch="arm64" ;;
    *) arch="$(uname -m)" ;;
  esac
fi

mkdir -p "$repo_root/$output_dir"

cd "$repo_root"
echo "Building release binary..."
cargo build -p velin-app --release

binary_path="$repo_root/target/release/velin-app"
portable_path="$repo_root/$output_dir/velin-app_${version}_${arch}"
cp "$binary_path" "$portable_path"

package_path="$repo_root/$output_dir/velin-app_${version}_${arch}.deb"
echo "Building Debian package at $package_path ..."
cargo deb -p velin-app --output "$package_path"

echo "Done: $package_path"
echo "Portable binary: $portable_path"
