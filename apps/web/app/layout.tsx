import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "zeronDesign",
  description: "Generate, preview, refine and publish websites and docs.",
};

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="zh-CN">
      <body>{children}</body>
    </html>
  );
}
