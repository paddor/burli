#!/bin/sh
set -e
cd "$(dirname "$0")"

PKG=src/pkg
TMP=src/pkg-tmp

rm -rf "$PKG" "$TMP"
mkdir -p "$PKG"

echo "==> Building WASM..."
cd wasm
wasm-pack build --target web --release --out-dir "../$TMP"
cd ..

# wasm-pack appends a blank line to the package license.
awk 'NF { last = NR } { lines[NR] = $0 } END { for (i = 1; i <= last; i++) print lines[i] }' \
  LICENSE > LICENSE.tmp
mv LICENSE.tmp LICENSE

cp "$TMP/burli_wasm.js" "$PKG/"
cp "$TMP/burli_wasm.d.ts" "$PKG/"
cp "$TMP/burli_wasm_bg.wasm.d.ts" "$PKG/"
mv "$TMP/burli_wasm_bg.wasm" "$PKG/"

rm -rf "$TMP"

WASM_SIZE=$(wc -c < "$PKG/burli_wasm_bg.wasm")
echo "==> Done. ${WASM_SIZE} bytes"
