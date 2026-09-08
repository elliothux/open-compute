import { resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  base: "/operator/",
  plugins: [
    tanstackRouter({
      target: "react",
      autoCodeSplitting: true,
      routesDirectory: resolve(import.meta.dirname, "src/routes"),
      generatedRouteTree: resolve(import.meta.dirname, "src/route-tree.gen.ts"),
    }),
    react(),
    tailwindcss(),
  ],
  build: {
    sourcemap: false,
    outDir: "dist",
    emptyOutDir: true,
  },
  server: {
    port: 5173,
  },
});
