#!/bin/bash
# Patches the dev build RPATH using NIX_LDFLAGS (set by nix develop / direnv).
# Must be run inside the nix dev environment.
set -e

BINARY="${1:-target/debug/keytao-installer}"

if [ -z "${NIX_LDFLAGS:-}" ]; then
  echo "ERROR: NIX_LDFLAGS is not set. Run inside 'nix develop' or with direnv." >&2
  exit 1
fi

RPATH=$(echo "$NIX_LDFLAGS" | tr ' ' '\n' | grep '^-L' | sed 's/^-L//' | sort -u | tr '\n' ':' | sed 's/:$//')
patchelf --set-rpath "$RPATH" "$BINARY"
echo "patchelf done: $BINARY"

echo "Missing libs:"
ldd "$BINARY" 2>&1 | grep "not found" || echo "  (none)"
