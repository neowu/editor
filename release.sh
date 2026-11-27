#!/usr/bin/env bash
# Builds the unsigned target/release/ed.app. Arguments pass through to `cargo build`
# (CI adds --no-default-features to precompile Metal shaders). install.sh signs it.
set -euo pipefail
cd "$(dirname "$0")"

version=$(sed -n '/^version = "/{s/^version = "\(.*\)"/\1/p;q;}' Cargo.toml)
if [ -z "$version" ]; then
    echo "Could not read workspace version from Cargo.toml" >&2
    exit 1
fi
resources=crates/ed/resources
app=target/release/ed.app
MACOSX_DEPLOYMENT_TARGET=$(plutil -extract LSMinimumSystemVersion raw $resources/Info.plist)
export MACOSX_DEPLOYMENT_TARGET

echo "Building ed $version"
cargo build --release --package ed "$@"

rm -rf $app
mkdir -p $app/Contents/MacOS $app/Contents/Resources

cp target/release/ed $app/Contents/MacOS/
cp $resources/Info.plist $app/Contents/Info.plist
plutil -replace CFBundleShortVersionString -string "$version" $app/Contents/Info.plist
plutil -replace CFBundleVersion -string "$version" $app/Contents/Info.plist

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
iconset=$tmp/ed.iconset
mkdir "$iconset"
for size in 16 32 128 256 512; do
    double=$((size * 2))
    sips -z $size $size $resources/app-icon@2x.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
    sips -z $double $double $resources/app-icon@2x.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o $app/Contents/Resources/ed.icns
cp $resources/Document.icns $app/Contents/Resources/

echo "Packaged $app"
