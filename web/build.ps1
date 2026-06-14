# Desolate Haven Cartography Engine - web build (PowerShell).
# Mirrors build.sh for Windows/PowerShell users:
#   1. license gate   2. (optional) Rust->WASM engine   3. esbuild bundle
# Run from the web/ directory:  .\build.ps1
# If blocked by execution policy:  powershell -ExecutionPolicy Bypass -File .\build.ps1
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $root
New-Item -ItemType Directory -Force -Path build | Out-Null

# 1. License gate (best-effort; skips checks whose tooling is absent).
node ../scripts/check-licenses.mjs
if ($LASTEXITCODE -ne 0) { throw "License gate failed" }

# 2. Build the Rust->WASM engine and copy artifacts into build/ (if wasm-pack present).
if (Get-Command wasm-pack -ErrorAction SilentlyContinue) {
  Push-Location ../crates/dhce-wasm
  wasm-pack build --release --target web --out-dir pkg
  Pop-Location
  Copy-Item ../crates/dhce-wasm/pkg/dhce_wasm_bg.wasm build/ -Force -ErrorAction SilentlyContinue
  Copy-Item ../crates/dhce-wasm/pkg/dhce_wasm.js     build/_engine.js -Force -ErrorAction SilentlyContinue
} else {
  Write-Host "note: wasm-pack not found - building the web shell with the TS placeholder engine."
}

# 3. Bundle the front-end (esbuild; types are checked by tsc / your editor, not here).
esbuild --bundle src/main.ts --format=esm --sourcemap --outfile=build/_bundle.js
Write-Host "build complete -> serve this directory (python -m http.server 8000) and open index.html"
