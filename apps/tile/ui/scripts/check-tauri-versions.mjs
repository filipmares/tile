// Fails when a Tauri npm package and its Rust crate drift apart by major/minor.
// `tauri build` refuses to bundle in that state, but nothing else in CI runs
// it, so without this check the mismatch first surfaces when a release tag is
// pushed (Dependabot updates the cargo and npm groups independently).
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..");
const cargoLock = readFileSync(resolve(root, "Cargo.lock"), "utf8");
const npmLock = JSON.parse(
  readFileSync(resolve(root, "apps/tile/ui/package-lock.json"), "utf8"),
);

const crates = new Map();
for (const block of cargoLock.split("[[package]]")) {
  const name = /^name = "([^"]+)"/m.exec(block)?.[1];
  const version = /^version = "([^"]+)"/m.exec(block)?.[1];
  if (name && version) crates.set(name, version);
}

const minor = (v) => v.split(".").slice(0, 2).join(".");
const mismatches = [];
let checked = 0;
for (const [path, pkg] of Object.entries(npmLock.packages ?? {})) {
  const npmName = /^node_modules\/@tauri-apps\/(api|plugin-[a-z-]+)$/.exec(path)?.[1];
  if (!npmName) continue;
  const crate = npmName === "api" ? "tauri" : `tauri-${npmName}`;
  const crateVersion = crates.get(crate);
  if (!crateVersion) continue;
  checked++;
  if (minor(crateVersion) !== minor(pkg.version)) {
    mismatches.push(`${crate} ${crateVersion} vs @tauri-apps/${npmName} ${pkg.version}`);
  }
}

if (checked === 0) {
  console.error("No Tauri npm/crate pairs found; check the lockfile paths.");
  process.exit(1);
}
if (mismatches.length > 0) {
  console.error("Tauri npm packages and Rust crates must share major.minor:");
  for (const m of mismatches) console.error(`  ${m}`);
  console.error(
    "Update only the lockfile in apps/tile/ui, keeping the ^2 ranges: " +
      "`npm install --package-lock-only @tauri-apps/<pkg>@~<major.minor>`, " +
      "then restore `^2` in package.json and package-lock.json.",
  );
  process.exit(1);
}
console.log(`Tauri npm packages match their crates (${checked} checked).`);
