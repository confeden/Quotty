#!/bin/sh
# Builds the Linux release binary and packs it with the install script:
#   sh tools/build-linux.sh
# Output: dist/Quotty-<version>_linux.tar.gz, the only Linux archive left in dist/.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

# Single source of truth for the version: Cargo.toml.
version=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n 1)
echo "Quotty $version"

# Own target dir: the Windows build keeps target/ in this folder, and mixing the
# two would make each sweep the other's units.
target="${CARGO_TARGET_DIR:-$HOME/.cache/quotty-linux-target}"
CARGO_TARGET_DIR="$target" cargo build --release

name="Quotty-${version}_linux"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/$name"

cp "$target/release/quotty" "$stage/$name/quotty"
cp assets/quotty.png "$stage/$name/quotty.png"
cp packaging/linux/install.sh packaging/linux/uninstall.sh "$stage/$name/"
cp README.md LICENSE "$stage/$name/"
chmod 755 "$stage/$name/quotty" "$stage/$name/install.sh" "$stage/$name/uninstall.sh"

mkdir -p dist
rm -f dist/Quotty-*_linux.tar.gz
tar -C "$stage" -czf "dist/$name.tar.gz" "$name"
echo "dist/$name.tar.gz  $(du -h "dist/$name.tar.gz" | cut -f1)"
