#!/usr/bin/env bash
# Copies the core binary to where Tauri's externalBin expects it (binaries/health-<target triple>).
set -euo pipefail
profile="${1:-release}"
root="$(cd "$(dirname "$0")/.." && pwd)"
triple="$(rustc -vV | sed -n 's/^host: //p')"
ext=""; [[ "$triple" == *windows* ]] && ext=".exe"
cargo build --manifest-path "$root/Cargo.toml" -p health $([[ "$profile" == release ]] && echo --release)
mkdir -p "$root/gui/src-tauri/binaries"
cp "$root/target/$profile/health$ext" "$root/gui/src-tauri/binaries/health-$triple$ext"
echo "staged health-$triple$ext ($profile)"
