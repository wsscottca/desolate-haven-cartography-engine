#!/bin/sh
# Desolate Haven Cartography Engine — web build.
#   1. license gate   2. (optional) Rust→WASM engine   3. esbuild bundle
set -e
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"
mkdir -p build

# 1. License gate (best-effort; skips checks whose tooling is absent).
node ../scripts/check-licenses.mjs

# 2. Build the Rust→WASM engine and copy artifacts into build/ (Phase 1+).
if command -v wasm-pack >/dev/null 2>&1; then
  ( cd ../crates/dhce-wasm && wasm-pack build --release --target web --out-dir pkg )
  cp ../crates/dhce-wasm/pkg/dhce_wasm_bg.wasm build/ 2>/dev/null || true
  cp ../crates/dhce-wasm/pkg/dhce_wasm.js     build/_engine.js 2>/dev/null || true
else
  echo "note: wasm-pack not found — building the web shell with the TS placeholder engine."
fi

# 3. Bundle the front-end (esbuild; types are checked by your editor / tsc, not here).
esbuild --bundle src/main.ts --format=esm --sourcemap --outfile=build/_bundle.js

echo "build complete → serve this directory and open index.html"
