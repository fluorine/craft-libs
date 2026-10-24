#!/usr/bin/env bash
# Separate every plate-test scene with Ghostscript (local only, never CI) and judge the plates.
# Ghostscript is `gs` on PATH, or CRAFT_PLATE_TEST_GS.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
fonts="${XDG_CACHE_HOME:-$HOME/.cache}/craft-pdf-test-fonts"
if [[ -z "${CRAFT_PDF_TEST_FONT_DIR:-}" && -f "$fonts/Inter-Regular.ttf" ]]; then
  export CRAFT_PDF_TEST_FONT_DIR="$fonts"
fi
cd "$root"
cargo test -p craft-plate-test --test ghostscript -- --ignored --nocapture
echo "report: ${CARGO_TARGET_DIR:-target}/plate-test/report.md"
