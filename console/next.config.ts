import type { NextConfig } from "next";

// M-T1 (Tauri repurpose): switched from `output: "standalone"` to
// `output: "export"` because Tauri serves the static `out/` directory;
// no Node.js server in production. `assetPrefix` is only set in dev so
// Next.js HMR assets load from the Vite/Next dev server origin
// (http://localhost:3000) instead of the Tauri webview origin.
// See docs/TAURI_APP_SPEC.md §5.1 and Appendix F.1.
const isProd = process.env.NODE_ENV === "production";
const internalHost = process.env.TAURI_DEV_HOST || "localhost";

const nextConfig: NextConfig = {
  output: "export",
  images: {
    unoptimized: true,
  },
  assetPrefix: isProd ? undefined : `http://${internalHost}:3000`,
  typescript: {
    ignoreBuildErrors: false, // P10 fix (M14): was true — masked v1 type drift.
  },
  reactStrictMode: false,
};

export default nextConfig;
