// Finish the standalone build: copy .next/static and public next to the
// located server.js entry (Turbopack roots the trace at the workspace when
// node_modules is shared/symlinked, nesting the entry — this works for both
// layouts).
//
// M-T1 (Tauri repurpose): with `output: 'export'` (the new default for the
// Tauri repurpose), Next.js produces `out/` and never creates
// `.next/standalone/`. Rather than maintain two build scripts in
// package.json (which is fragile on Windows shells), we make this script a
// friendly no-op when the standalone dir is absent — so `npm run build`
// stays `next build && node scripts/finish-standalone.mjs` and works in both
// modes. If a future milestone goes back to `output: 'standalone'`, this
// script will resume its real work automatically.
import { cpSync, existsSync, mkdirSync, readdirSync, statSync } from "node:fs";
import { dirname, join } from "node:path";

const root = process.cwd();
const standalone = join(root, ".next", "standalone");

if (!existsSync(standalone)) {
  console.log(
    "finish-standalone: .next/standalone/ not found (output: 'export' mode) — skipping.",
  );
  process.exit(0);
}

function findServer(dir) {
  for (const name of readdirSync(dir)) {
    if (name === "node_modules") continue;
    const p = join(dir, name);
    const st = statSync(p);
    if (st.isDirectory()) {
      const hit = findServer(p);
      if (hit) return hit;
    } else if (name === "server.js") {
      return p;
    }
  }
  return null;
}

const server = findServer(standalone);
if (!server) {
  console.error("finish-standalone: no server.js under .next/standalone");
  process.exit(1);
}
const serverDir = dirname(server);
mkdirSync(join(serverDir, ".next"), { recursive: true });
cpSync(join(root, ".next", "static"), join(serverDir, ".next", "static"), { recursive: true });
if (existsSync(join(root, "public"))) {
  cpSync(join(root, "public"), join(serverDir, "public"), { recursive: true });
}
console.log("standalone entry:", server);
