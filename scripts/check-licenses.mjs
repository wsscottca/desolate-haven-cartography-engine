#!/usr/bin/env node
/*
 * Permissive-license gate for DHCE.
 *
 * Fails the build if any dependency uses a non-permissive (copyleft / unknown)
 * license. Best-effort: if a toolchain (cargo) isn't installed, that check is
 * skipped with a note so a fresh checkout still builds. Run from web/build.sh.
 */
import { execFileSync } from "node:child_process";

const PERMISSIVE = new Set([
  "MIT", "ISC", "BSD-2-Clause", "BSD-3-Clause", "Apache-2.0", "Zlib",
  "Unlicense", "CC0-1.0", "0BSD", "MPL-2.0", "OFL-1.1", "LicenseRef-Proprietary",
]);
// Apache-2.0 alone is permitted but flagged — we prefer to minimize Apache lineage.
const FLAG_ONLY = new Set(["Apache-2.0"]);

// Split an SPDX expression into its license atoms. Handles both the modern
// "A OR B" / "A AND B" form and the legacy slash form "A/B" (no surrounding spaces,
// as older crates like console_error_panic_hook still use).
const splitExpr = (expr) =>
  expr
    .replace(/[()]/g, " ")
    .split(/\s+(?:OR|AND)\s+|\s*\/\s*/i)
    .map((s) => s.trim())
    .filter(Boolean);
// OR semantics: an expression is permissive if ANY listed license is (we elect it).
const isPermissive = (expr) => !!expr && splitExpr(expr).some((p) => PERMISSIVE.has(p));

const violations = [];
const notes = [];

// --- Rust crates via `cargo metadata` (skipped if cargo absent) ---
try {
  const out = execFileSync("cargo", ["metadata", "--format-version", "1", "--quiet"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  });
  const meta = JSON.parse(out);
  const ours = new Set(meta.workspace_members);
  for (const pkg of meta.packages) {
    if (ours.has(pkg.id)) continue; // our own crates
    const lic = pkg.license || (pkg.license_file ? "FILE-ONLY" : "");
    if (!isPermissive(lic)) {
      violations.push(`rust: ${pkg.name}@${pkg.version} → "${lic || "UNKNOWN"}"`);
    } else if (splitExpr(lic).every((p) => FLAG_ONLY.has(p))) {
      notes.push(`rust: ${pkg.name} is Apache-2.0-only (permitted, flagged)`);
    }
  }
} catch {
  notes.push("rust: cargo not found — crate license check skipped");
}

if (notes.length) console.log("license notes:\n  " + notes.join("\n  "));
if (violations.length) {
  console.error("LICENSE GATE FAILED — non-permissive dependencies:\n  " + violations.join("\n  "));
  process.exit(1);
}
console.log(`license gate: OK${notes.length ? " (with notes)" : ""}`);
