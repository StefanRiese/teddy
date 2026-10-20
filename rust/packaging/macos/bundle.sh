#!/bin/sh
# Builds TeddyBench.app and the teddy CLI as universal binaries (Apple Silicon + Intel)
# and packs them into a disk image. Run on macOS from the rust/ directory:
#   packaging/macos/bundle.sh <output.dmg>
set -eu

out="$1"
here="$(cd "$(dirname "$0")" && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
targets="aarch64-apple-darwin x86_64-apple-darwin"
export MACOSX_DEPLOYMENT_TARGET=11.0

for t in $targets; do
    rustup target add "$t" > /dev/null
    cargo build --release --target "$t" -p teddybench -p teddy
done

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
stage="$work/TeddyBench"
app="$stage/TeddyBench.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

for bin in teddybench teddy; do
    lipo -create -output "$work/$bin" target/aarch64-apple-darwin/release/$bin target/x86_64-apple-darwin/release/$bin
done
mv "$work/teddybench" "$app/Contents/MacOS/teddybench"
mv "$work/teddy" "$stage/teddy"
sed "s/@VERSION@/$version/g" "$here/Info.plist" > "$app/Contents/Info.plist"

# icon from the app's own image
iconset="$work/TeddyBench.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z $size $size teddybench/assets/custom.png --out "$iconset/icon_${size}x${size}.png" > /dev/null
    double=$((size * 2))
    sips -z $double $double teddybench/assets/custom.png --out "$iconset/icon_${size}x${size}@2x.png" > /dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/TeddyBench.icns"

# ad-hoc signature, required for Apple Silicon; not a Developer ID, so Gatekeeper still asks once
codesign --force --deep --sign - "$app"
codesign --force --sign - "$stage/teddy"

cp README.md teddy/LICENSES.txt "$stage/"
ln -s /Applications "$stage/Applications"

lipo -info "$app/Contents/MacOS/teddybench"
rm -f "$out"
hdiutil create -volname "TeddyBench" -srcfolder "$stage" -format UDZO -ov "$out" > /dev/null
echo "Created $out"
