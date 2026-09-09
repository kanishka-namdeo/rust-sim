// Copy the MapLibre GL worker siblings into public/maplibre/ so Next.js's
// standalone bundler can serve them at a stable URL. This is the
// officially-documented Next.js / Turbopack workaround: under both bundler
// modes `new URL(..., import.meta.url)` drops the `maplibre-gl-shared.mjs`
// sibling, so the map mounts but never loads a tile.
//
// Spec: docs/GCS_V2_SPEC.md §6.2 — copies BOTH siblings. Runs as `predev`
// and `prebuild`; `public/maplibre/` is gitignored (build artifact, never
// committed). `finish-standalone.mjs` then copies `public/` into
// `.next/standalone/` so the production server also serves them.
//
// M8 / T-A1.
import { cpSync, mkdirSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const root = resolve(__dirname, "..");
const srcDir = join(root, "node_modules", "maplibre-gl", "dist");
const dstDir = join(root, "public", "maplibre");

if (!statSync(srcDir, { throwIfNoEntry: false })) {
  console.error("copy-maplibre-worker: maplibre-gl not installed; run `npm install` first");
  process.exit(1);
}

mkdirSync(dstDir, { recursive: true });

// Per the spec §6.1 the v6 ESM-only bundle ships both these siblings.
// Copy both, plus the LICENSE so G-14 can assert BSD-3 in the bundle.
// The .mjs siblings live in dist/; the LICENSE.txt is at the package root.
const toCopy = [
  ["maplibre-gl-worker.mjs", srcDir],
  ["maplibre-gl-shared.mjs", srcDir],
  ["LICENSE.txt", join(root, "node_modules", "maplibre-gl")],
];

let copied = 0;
for (const [f, fromDir] of toCopy) {
  const src = join(fromDir, f);
  if (statSync(src, { throwIfNoEntry: false })) {
    cpSync(src, join(dstDir, f));
    copied++;
  } else if (f !== "LICENSE.txt") {
    // LICENSE.txt is optional — some packagings may not include it. The two
    // .mjs siblings are required; if either is missing the map will not tile.
    console.error(`copy-maplibre-worker: required sibling ${f} missing in ${fromDir}`);
    process.exit(1);
  }
}

// Sanity check: log what landed.
const ls = readdirSync(dstDir).sort();
console.log(`copy-maplibre-worker: ${copied} files -> public/maplibre/ [${ls.join(", ")}]`);
