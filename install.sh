#!/usr/bin/env bash
# Signs ed.app and installs it to /Applications, for local use only.
#   ./install.sh          installs target/release/ed.app built by release.sh
#   ./install.sh x.y.z    installs the GitHub release of that version
# Signs with a self-signed identity (default `ed-local`, override with
# ED_SIGN_IDENTITY) so macOS privacy grants such as Local Network survive updates.
set -euo pipefail
cd "$(dirname "$0")"

identity=${ED_SIGN_IDENTITY:-ed-local}
dest=/Applications/ed.app

if ! security find-certificate -c "$identity" >/dev/null 2>&1; then
    cat >&2 <<EOF
Code signing identity '$identity' not found in the keychain. Create it once with:

    mkdir /tmp/ed-sign && cd /tmp/ed-sign
    /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj /CN=$identity \\
        -addext keyUsage=critical,digitalSignature -addext extendedKeyUsage=critical,codeSigning \\
        -keyout key.pem -out cert.pem
    /usr/bin/openssl pkcs12 -export -inkey key.pem -in cert.pem -out id.p12 -passout pass:ed
    security import id.p12 -k ~/Library/Keychains/login.keychain-db -P ed -T /usr/bin/codesign
    cd - && rm -rf /tmp/ed-sign
EOF
    exit 1
fi

if [ $# -gt 0 ]; then
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    echo "Downloading ed $1"
    curl -fL -o "$tmp/ed.zip" "https://github.com/neowu/editor/releases/download/$1/ed-macos-arm64.zip"
    ditto -x -k "$tmp/ed.zip" "$tmp"
    app=$tmp/ed.app
else
    app=target/release/ed.app
    if [ ! -d $app ]; then
        echo "$app not found, run ./release.sh first" >&2
        exit 1
    fi
fi

codesign --force --sign "$identity" "$app"
rm -rf $dest
mv "$app" $dest

echo "Installed $dest"
