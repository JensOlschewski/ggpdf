#!/usr/bin/env bash
# Builds assets/test-pdfs/readme.pdf from README.md: the document ggPdf opens when
# started without a file. Rerun after editing README.md.
# Usage: scripts/readme-pdf.sh [output.pdf]
# Needs pandoc and Google Chrome (or Chromium; set CHROME to its binary).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/assets/test-pdfs/readme.pdf}"

chrome="${CHROME:-}"
if [ -z "$chrome" ]; then
  for candidate in \
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
    "$(command -v google-chrome || true)" \
    "$(command -v chromium || true)"; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then chrome="$candidate"; break; fi
  done
fi
command -v pandoc >/dev/null || { echo "pandoc not found" >&2; exit 1; }
[ -n "$chrome" ] || { echo "Chrome/Chromium not found (set CHROME)" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Self-contained HTML: images and CSS are inlined, so the PDF has them too.
cd "$root"
pandoc README.md \
  --from gfm \
  --to html5 \
  --standalone \
  --embed-resources \
  --resource-path "$root" \
  --css "$root/scripts/readme-pdf.css" \
  --metadata pagetitle="ggPdf" \
  --output "$work/readme.html"

"$chrome" --headless --disable-gpu --no-pdf-header-footer \
  --print-to-pdf="$out" "file://$work/readme.html" 2>/dev/null
echo "wrote $out"
