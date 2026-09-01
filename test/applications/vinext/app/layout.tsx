import type { Metadata } from "next";
import type { ReactNode } from "react";

export const metadata: Metadata = {
  title: "open-compute P4 vinext qualification",
  description: "Fixed Next.js 16 production workload for Cloudflare Workers qualification",
};

export default function RootLayout({ children }: { readonly children: ReactNode }) {
  return (
    <html lang="en">
      <body data-qualification="p4-vinext">{children}</body>
    </html>
  );
}
