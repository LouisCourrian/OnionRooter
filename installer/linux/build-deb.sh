#!/usr/bin/env bash
# Build a Debian package for the OnionRouter Native Messaging companion.
#
# The package installs only the companion and the Firefox Native Messaging
# host manifest. The Firefox extension is distributed separately through AMO
# or as a signed XPI.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTPUT_DIR="${1:-"$REPO_ROOT/dist"}"

VERSION="$(
  python3 - "$REPO_ROOT/extension/manifest.json" <<'PY'
import json
import sys
with open(sys.argv[1], encoding="utf-8") as f:
    print(json.load(f)["version"])
PY
)"

case "$(uname -m)" in
  x86_64) DEB_ARCH="amd64" ;;
  *)
    echo "Unsupported Debian package architecture: $(uname -m)" >&2
    echo "The bundled Tor Expert Bundle is currently pinned for linux-x86_64 only." >&2
    exit 1
    ;;
esac

PACKAGE="onionrouter-companion"
BUILD_ROOT="$REPO_ROOT/installer/build/deb"
STAGE="$BUILD_ROOT/${PACKAGE}_${VERSION}_${DEB_ARCH}"
DEB_PATH="$OUTPUT_DIR/${PACKAGE}_${VERSION}_${DEB_ARCH}.deb"

mkdir -p "$OUTPUT_DIR"
rm -rf "$STAGE"
mkdir -p "$STAGE"

cargo build --manifest-path "$REPO_ROOT/companion/Cargo.toml" --release

install -D -m 0755 \
  "$REPO_ROOT/companion/target/release/onionrouter-companion" \
  "$STAGE/usr/lib/onionrouter/onionrouter-companion"

install -d -m 0755 "$STAGE/usr/lib/mozilla/native-messaging-hosts"
cat > "$STAGE/usr/lib/mozilla/native-messaging-hosts/com.onionrouter.companion.json" <<'JSON'
{
  "name": "com.onionrouter.companion",
  "description": "OnionRouter Tor management companion",
  "path": "/usr/lib/onionrouter/onionrouter-companion",
  "type": "stdio",
  "allowed_extensions": ["onionrouter@louis-courrian.dev"]
}
JSON

install -d -m 0755 "$STAGE/usr/share/doc/$PACKAGE"
cat > "$STAGE/usr/share/doc/$PACKAGE/README.Debian" <<EOF
OnionRouter companion for Debian/Ubuntu
=======================================

This package installs the Native Messaging companion and registers it
system-wide for Firefox.

Install the Firefox extension separately through addons.mozilla.org or with a
signed XPI. Unsigned XPI files are not accepted by standard Firefox releases.

The companion downloads the official Tor Expert Bundle on first use, verifies
the pinned SHA-256 hash, and stores Tor data in the user's local data directory.
EOF

cat > "$STAGE/usr/share/doc/$PACKAGE/copyright" <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: OnionRouter
Source: https://github.com/LouisCourrian/OnionRooter

Files: *
Copyright: Louis COURRIAN
License: MIT

License: MIT
 Permission is hereby granted, free of charge, to any person obtaining a copy
 of this software and associated documentation files (the "Software"), to deal
 in the Software without restriction, including without limitation the rights
 to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 copies of the Software, and to permit persons to whom the Software is
 furnished to do so, subject to the following conditions:
 .
 The above copyright notice and this permission notice shall be included in all
 copies or substantial portions of the Software.
 .
 THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 SOFTWARE.
EOF

install -d -m 0755 "$STAGE/DEBIAN"
cat > "$STAGE/DEBIAN/control" <<EOF
Package: $PACKAGE
Version: $VERSION
Section: net
Priority: optional
Architecture: $DEB_ARCH
Maintainer: Louis COURRIAN <louis.courrian@gmail.com>
Homepage: https://github.com/LouisCourrian/OnionRooter
Depends: libc6 (>= 2.35), libgcc-s1 (>= 3.0)
Recommends: firefox | firefox-esr
Description: Native Messaging companion for OnionRouter
 OnionRouter lets Firefox route .onion traffic through Tor without requiring
 the user to install Tor Browser. This package installs the Rust companion
 used by the Firefox extension to download, verify, launch, and reuse Tor.
EOF

dpkg-deb --build --root-owner-group "$STAGE" "$DEB_PATH"
echo "$DEB_PATH"
