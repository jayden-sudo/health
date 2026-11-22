#!/usr/bin/env bash
# Plan §2.2: only health-connect may open network connections. No other crate may depend on a networking
# crate; the binary may reach one only through health-connect (pruned from its tree here).
# zbus (Linux Secret Service) talks D-Bus over a local socket; that is IPC, not network.
set -euo pipefail
banned='^(reqwest|hyper|ureq|isahc|curl|surf|attohttpc|tokio|async-std|mio|socket2|h2|rustls|native-tls|openssl-probe|tungstenite) '
for crate in health-core health-api health-ocr health; do
  prune=()
  [ "$crate" = health ] && prune=(--prune health-connect)
  tree=$(cargo tree -p "$crate" -e normal --target all --prefix none --format '{p}' ${prune[@]+"${prune[@]}"})
  if sort -u <<<"$tree" | grep -E "$banned"; then
    echo "error: $crate depends on a networking crate (only health-connect may)" >&2
    exit 1
  fi
done
echo "no networking crates outside health-connect"
