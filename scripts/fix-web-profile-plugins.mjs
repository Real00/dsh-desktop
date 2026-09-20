#!/usr/bin/env node
/**
 * Recovery: clear non-core plugins from ~/.dsh/profiles/web/package.json
 * - Backs up package.json to a timestamped .bak
 * - Sets dependencies to {}
 * - Sets dsh.profile.bundles to only core: @deepseek-ai/dsh-base, @deepseek-ai/dsh-web-app
 * Safe to re-run. Does not touch anything outside the web profile package.json.
 *
 * Usage (macOS/Linux):
 *   node /path/to/fix-web-profile-plugins.mjs
 *   node /path/to/fix-web-profile-plugins.mjs /custom/path/to/package.json
 */
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const CORE = ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"];

function timestamp() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, "0");
  return (
    d.getFullYear() +
    p(d.getMonth() + 1) +
    p(d.getDate()) +
    "-" +
    p(d.getHours()) +
    p(d.getMinutes()) +
    p(d.getSeconds())
  );
}

function defaultPkgPath() {
  return path.join(os.homedir(), ".dsh", "profiles", "web", "package.json");
}

function main() {
  const pkgPath = process.argv[2] || defaultPkgPath();
  console.log("Target:", pkgPath);

  if (!fs.existsSync(pkgPath)) {
    console.error("ERROR: package.json not found. Nothing to do.");
    process.exit(1);
  }

  const beforeText = fs.readFileSync(pkgPath, "utf8");
  let before;
  try {
    before = JSON.parse(beforeText);
  } catch (e) {
    console.error("ERROR: invalid JSON:", e.message);
    process.exit(1);
  }

  const bakPath = `${pkgPath}.bak.${timestamp()}`;
  fs.writeFileSync(bakPath, beforeText);
  console.log("Backup:", bakPath);

  console.log("\n=== BEFORE ===");
  console.log("dependencies:", JSON.stringify(before.dependencies || {}, null, 2));
  console.log(
    "dsh.profile.bundles:",
    JSON.stringify(before?.dsh?.profile?.bundles ?? null, null, 2),
  );

  const after = structuredClone(before);
  after.dependencies = {};
  if (!after.dsh || typeof after.dsh !== "object") after.dsh = {};
  if (!after.dsh.profile || typeof after.dsh.profile !== "object") {
    after.dsh.profile = {};
  }
  after.dsh.profile.bundles = [...CORE];
  // Clear similar list if present
  if (Array.isArray(after.dsh.profile.plugins)) {
    after.dsh.profile.plugins = after.dsh.profile.plugins.filter((id) =>
      CORE.some((c) => c.toLowerCase() === String(id).toLowerCase()),
    );
  }

  const out = JSON.stringify(after, null, 2) + "\n";
  fs.writeFileSync(pkgPath, out);

  console.log("\n=== AFTER ===");
  console.log("dependencies:", JSON.stringify(after.dependencies, null, 2));
  console.log("dsh.profile.bundles:", JSON.stringify(after.dsh.profile.bundles, null, 2));
  console.log("\nOK. Restart DSH Desktop / dsh. Re-run is safe.");
}

main();
