#!/usr/bin/env bash
# Render the registry server.json for a published MCPB bundle.
#
# usage: render-server-json.sh <template> <mcpb-url> <mcpb-sha256> <output>
#
# Replaces __MCPB_URL__ and __MCPB_SHA256__ in the template, fails if either
# placeholder is missing from the template or still present after substitution,
# validates the result parses as JSON, and writes it pretty-printed.
set -euo pipefail

usage() {
  printf 'usage: %s <template> <mcpb-url> <mcpb-sha256> <output>\n' "$0" >&2
  exit 2
}

[ $# -eq 4 ] || usage
template=$1
url=$2
sha256=$3
out=$4

[ -f "$template" ] || {
  echo "error: template '$template' is not a file" >&2
  exit 1
}
[ -n "$url" ] || {
  echo "error: mcpb-url must not be empty" >&2
  exit 1
}
printf '%s' "$sha256" | grep -Eq '^[0-9a-fA-F]{64}$' || {
  echo "error: mcpb-sha256 must be a 64-character hex digest, got '$sha256'" >&2
  exit 1
}

python3 - "$template" "$out" "$url" "$sha256" <<'EOF'
import json
import sys

template_path, out_path, url, sha256 = sys.argv[1:5]
with open(template_path, encoding="utf-8") as fh:
    text = fh.read()

for placeholder, value in (("__MCPB_URL__", url), ("__MCPB_SHA256__", sha256)):
    if placeholder not in text:
        sys.exit(f"error: {placeholder} not found in {template_path}")
    text = text.replace(placeholder, value)

for placeholder in ("__MCPB_URL__", "__MCPB_SHA256__"):
    if placeholder in text:
        sys.exit(f"error: {placeholder} still present after substitution")

try:
    data = json.loads(text)
except json.JSONDecodeError as exc:
    sys.exit(f"error: rendered server.json is not valid JSON: {exc}")

with open(out_path, "w", encoding="utf-8") as fh:
    json.dump(data, fh, indent=2)
    fh.write("\n")
EOF

echo "wrote $out"
