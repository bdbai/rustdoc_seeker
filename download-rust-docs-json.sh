#!/bin/bash

set -euo pipefail

OUT_DIR="doc-json"
TOOLCHAIN_METADATA_FILENAME="channel-rust-nightly.toml"
ARCHIVE_NAME="rust-docs-json-preview.tar.xz"

cd $OUT_DIR

if [ -z "${1:-}" ]; then
  echo using latest nightly
  URL="https://static.rust-lang.org/dist/channel-rust-nightly.toml"
else
  TOOLCHAIN_VERSION=$1
  echo using nightly toolchain version $TOOLCHAIN_VERSION
  URL="https://static.rust-lang.org/dist/${TOOLCHAIN_VERSION}/channel-rust-nightly.toml"
fi

curl -f -o "$TOOLCHAIN_METADATA_FILENAME" -s $URL
curl -f -o "$TOOLCHAIN_METADATA_FILENAME.sha256" -s $URL.sha256
sha256sum -c $TOOLCHAIN_METADATA_FILENAME.sha256
METADATA=$(cat "$TOOLCHAIN_METADATA_FILENAME")

LINES=$(echo "$METADATA" | grep -A 6 '\[pkg.rust-docs-json-preview\.target\.x86_64-unknown-linux-gnu\]')

XZ_URL=$(echo "$LINES" | grep 'xz_url' | cut -d '"' -f 2)
XZ_HASH=$(echo "$LINES" | grep 'xz_hash' | cut -d '"' -f 2)

curl -f -o $ARCHIVE_NAME $XZ_URL
echo "$XZ_HASH $ARCHIVE_NAME" | sha256sum -c -

tar --wildcards -xJf $ARCHIVE_NAME --strip-components=6 -C . "rust-docs-json-nightly-x86_64-unknown-linux-gnu/rust-docs-json-preview/share/doc/rust/json/*"

echo "Files successfully unarchived to $OUT_DIR"
