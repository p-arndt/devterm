// Stamp / bump the DevTerm workspace version.
//
//   node scripts/set-version.mjs 0.2.0     # set an explicit version
//   node scripts/set-version.mjs patch     # bump 0.1.3 -> 0.1.4
//   node scripts/set-version.mjs minor     # bump 0.1.3 -> 0.2.0
//   node scripts/set-version.mjs major     # bump 0.1.3 -> 1.0.0
//
// DevTerm keeps its version in ONE place: the root Cargo.toml [workspace.package]
// `version` key. Every crate inherits it via `version.workspace = true`, so this
// rewrites a single line via a targeted regex (not TOML round-tripping) — existing
// formatting, key order, and comments are left untouched. Cargo.lock refreshes
// itself on the next build since the workspace crates are path deps.
//
// Also exports readVersion / bumpVersion / setVersion / resolveVersion for
// scripts/release.mjs.

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join } from "node:path";

// Repo root is one level up from this script's scripts/ directory.
const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const cargoToml = join(root, "Cargo.toml");

// The (?m)^ anchor hits the [workspace.package] `version = "..."` — never
// `rust-version = "..."` or the inline dependency `version = "..."` entries.
// Group 1 captures the "version = " prefix so `$1"<version>"` swaps only the value.
const VERSION_RE = /^(version = )"[^"]*"/m;

/** Read the current workspace version from Cargo.toml. */
export function readVersion() {
  const m = VERSION_RE.exec(readFileSync(cargoToml, "utf8"));
  if (!m) throw new Error("could not find the workspace version in Cargo.toml");
  return m[0].match(/"([^"]*)"/)[1];
}

/** Bump a semver string by "patch" | "minor" | "major". */
export function bumpVersion(current, kind) {
  const m = /^(\d+)\.(\d+)\.(\d+)$/.exec(current);
  if (!m) throw new Error(`current version is not plain semver: ${current}`);
  let [major, minor, patch] = m.slice(1).map(Number);
  if (kind === "major") [major, minor, patch] = [major + 1, 0, 0];
  else if (kind === "minor") [minor, patch] = [minor + 1, 0];
  else if (kind === "patch") patch++;
  else throw new Error(`unknown bump "${kind}" (use patch|minor|major)`);
  return `${major}.${minor}.${patch}`;
}

/** Write `version` into the workspace Cargo.toml. Throws if the pattern is stale. */
export function setVersion(version) {
  if (!/^\d+\.\d+\.\d+/.test(version))
    throw new Error(`invalid version "${version}" (expected x.y.z)`);
  const before = readFileSync(cargoToml, "utf8");
  const after = before.replace(VERSION_RE, `$1"${version}"`);
  if (after === before)
    throw new Error("no version match in Cargo.toml — pattern may be stale");
  writeFileSync(cargoToml, after);
  console.log(`  Cargo.toml`);
  console.log(`Stamped version ${version}.`);
}

// Resolve a CLI argument to a concrete version: a bump keyword or an explicit x.y.z.
export function resolveVersion(arg) {
  return ["patch", "minor", "major"].includes(arg)
    ? bumpVersion(readVersion(), arg)
    : arg;
}

// CLI entry point (only when run directly, not when imported).
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const arg = process.argv[2];
  if (!arg) {
    console.error("usage: node scripts/set-version.mjs <patch|minor|major|x.y.z>");
    process.exit(1);
  }
  setVersion(resolveVersion(arg));
}
