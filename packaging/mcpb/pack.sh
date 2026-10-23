#!/usr/bin/env bash
# Pack a release binary into a deterministic MCPB bundle.
#
# usage: pack.sh <binary> <output-dir-or-file>
#
# The bundle layout is:
#   manifest.json        (copied from this script's directory)
#   server/mc3000-mcp    (the release binary)
#
# With a directory argument the bundle is written as
# <version>-x86_64-unknown-linux-gnu.mcpb inside that directory, where <version>
# is read from manifest.json. Prints "<sha256>  <output-path>" on success.
set -euo pipefail

usage() {
  printf 'usage: %s <binary> <output-dir-or-file>\n' "$0" >&2
  exit 2
}

[ $# -eq 2 ] || usage
bin=$1
out=$2

[ -f "$bin" ] && [ -x "$bin" ] || {
  echo "error: '$bin' is not an executable file" >&2
  exit 1
}

command -v zip >/dev/null 2>&1 || {
  echo "error: zip(1) is required but was not found" >&2
  exit 1
}

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
manifest=$script_dir/manifest.json
[ -f "$manifest" ] || {
  echo "error: manifest.json not found next to $0" >&2
  exit 1
}

version=$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$manifest" | head -n 1)
[ -n "$version" ] || {
  echo "error: could not read \"version\" from $manifest" >&2
  exit 1
}

case $out in
/*) ;;
*) out=$PWD/$out ;;
esac
if [ -d "$out" ]; then
  out=$out/${version}-x86_64-unknown-linux-gnu.mcpb
fi

staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
mkdir -p "$staging/server"
cp "$bin" "$staging/server/mc3000-mcp"
chmod 0755 "$staging/server/mc3000-mcp"
cp "$manifest" "$staging/manifest.json"

# Deterministic archive: fixed mtimes, fixed entry order, no extra attributes.
TZ=UTC find "$staging" -exec touch -h -t 198001010000.00 {} +
( cd "$staging" && TZ=UTC zip -q -X "$out" manifest.json server/mc3000-mcp )

sha256sum "$out"
