#!/bin/bash
# Build and verify a drag-to-Applications disk image on a native macOS runner.
set -euo pipefail

if [[ $# != 2 ]]; then
  echo "usage: bash packaging/macos/package.sh <rust-target> <release-ref>" >&2
  exit 2
fi
if [[ $(uname -s) != Darwin ]]; then
  echo "bro: disk images must be packaged on macOS" >&2
  exit 1
fi
target=$1
case "$target" in
  aarch64-apple-darwin) arch=arm64 ;;
  x86_64-apple-darwin) arch=x86_64 ;;
  *) echo "bro: unsupported macOS target: $target" >&2; exit 2 ;;
esac
if [[ $(uname -m) != "$arch" ]]; then
  echo "bro: use a native $arch runner so the packaged executable can be tested" >&2
  exit 1
fi

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
# A manually dispatched workflow may use a branch name containing slashes.
release_ref=$(printf '%s' "$2" | LC_ALL=C tr -c 'A-Za-z0-9._-' '-')
[[ -n "$release_ref" ]] || { echo "bro: empty release ref" >&2; exit 2; }
name="bro-$release_ref-$target"
binary="${CARGO_TARGET_DIR:-target}/$target/release/bro"
lipo "$binary" -verify_arch "$arch"
version=$("$binary" --version)
version=${version#bro }
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][A-Za-z0-9.-]+)?$ ]] || {
  echo "bro: unexpected executable version: $version" >&2
  exit 1
}
if [[ $release_ref == v[0-9]* && $release_ref != "v$version" ]]; then
  echo "bro: release $release_ref does not match executable version $version" >&2
  exit 1
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/bro-dmg.XXXXXX")
mounted=false
cleanup() {
  if $mounted; then hdiutil detach "$work/mounted" >/dev/null; fi
  rm -rf "$work"
}
trap cleanup EXIT
app="$work/image/Bro.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$work/Bro.iconset" dist
cp "$binary" "$app/Contents/MacOS/bro"
cp packaging/bro-cli "$app/Contents/MacOS/bro-cli"
chmod 755 "$app/Contents/MacOS/bro" "$app/Contents/MacOS/bro-cli"
cp packaging/macos/Info.plist "$app/Contents/Info.plist"
# Apple bundle versions use the numeric portion of Cargo's version.
bundle_version=${version%%[-+]*}
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $bundle_version" "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $bundle_version" "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
cp README.md LICENSE "$app/Contents/Resources/"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" crates/bro-tui/assets/bro.png --out "$work/Bro.iconset/icon_${size}x${size}.png" >/dev/null
  retina=$((size * 2))
  sips -z "$retina" "$retina" crates/bro-tui/assets/bro.png --out "$work/Bro.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$work/Bro.iconset" -o "$app/Contents/Resources/Bro.icns"
# Ad-hoc signing seals the bundle and supports Apple Silicon execution. Developer ID
# signing and notarization require Apple credentials, which are not configured here.
codesign --force --sign - "$app"
codesign --verify --deep --strict --verbose=2 "$app"
ln -s /Applications "$work/image/Applications"
hdiutil create -volname "Bro $version" -srcfolder "$work/image" -format UDZO -ov "dist/$name.dmg"
hdiutil verify "dist/$name.dmg"

# Check the delivered image, including its signature and exact executable bytes.
mkdir "$work/mounted"
hdiutil attach "dist/$name.dmg" -readonly -nobrowse -mountpoint "$work/mounted" >/dev/null
mounted=true
delivered="$work/mounted/Bro.app"
codesign --verify --deep --strict --verbose=2 "$delivered"
test "$(readlink "$work/mounted/Applications")" = /Applications
test -s "$delivered/Contents/Resources/Bro.icns"
cmp "$app/Contents/MacOS/bro" "$delivered/Contents/MacOS/bro"
test "$("$delivered/Contents/MacOS/bro" --version)" = "bro $version"
# Exercise the GUI from the mounted app in demo mode, with a deadline for startup failures.
python3 - "$delivered/Contents/MacOS/bro" "$work/smoke.png" <<'PY'
import os
from pathlib import Path
import subprocess
import sys

binary, screenshot = sys.argv[1:]
env = dict(os.environ, BRO_GUI_SCRIPT=f"wait:2000;shot:{screenshot};exit")
subprocess.run([binary, "gui", "--gui-child", "--demo"], env=env, check=True, timeout=60)
assert Path(screenshot).read_bytes().startswith(b"\x89PNG\r\n\x1a\n"), "Bro did not render a GUI frame"
print("Verified the mounted app renders its GUI")
PY
echo "Verified dist/$name.dmg ($arch, bro $version)"
