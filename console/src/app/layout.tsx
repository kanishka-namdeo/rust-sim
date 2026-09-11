import type { Metadata } from "next";
import localFont from "next/font/local";
import "./globals.css";
import { Providers } from "@/components/providers";

// M-T1 (Tauri repurpose): switched from `next/font/google` to
// `next/font/local` with vendored variable-axis .ttf in public/fonts/
// (extracted from the `geist` npm package v1.7.2) so the build works in
// air-gapped CI (no fonts.gstatic.com fetch at build time).
// Note: next/font/local resolves `src` relative to THIS file (src/app/),
// so reaching the project-root `public/fonts/` requires `../../public/`.
// (An earlier draft used `../public/fonts/` which resolved to the
// nonexistent `src/public/fonts/` and broke `next build`.)
// See docs/TAURI_APP_SPEC.md §5.1 and Appendix F.2/F.8.
const geistSans = localFont({
  src: "../../public/fonts/Geist-Variable.ttf",
  display: "swap",
  variable: "--font-geist-sans",
});

const geistMono = localFont({
  src: "../../public/fonts/GeistMono-Variable.ttf",
  display: "swap",
  variable: "--font-geist-mono",
});

export const metadata: Metadata = {
  title: "Operations Canvas — rustsitsim · mavfleet",
  description:
    "GCS v2 single-screen operator surface (MapLibre + edge HUD) for PX4 SITL.",
  keywords: ["PX4", "HIL", "SITL", "MAVLink", "rustsitsim", "mavfleet", "fleet manager", "telemetry", "operator console"],
  icons: {
    // M-T1: was https://z-cdn.chatglm.cn/z-ai/static/logo.svg (fails offline).
    icon: "/logo.svg",
  },
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body
        className={`${geistSans.variable} ${geistMono.variable} antialiased bg-background text-foreground`}
      >
        <Providers>
          {children}
        </Providers>
      </body>
    </html>
  );
}
