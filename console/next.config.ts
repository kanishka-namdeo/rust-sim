import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  output: "standalone",
  typescript: {
    ignoreBuildErrors: false, // P10 fix (M14): was true — masked v1 type drift.
  },
  reactStrictMode: false,
};

export default nextConfig;
