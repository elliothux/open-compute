import { resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const devApiOrigin = process.env.OPEN_COMPUTE_DASHBOARD_DEV_API_ORIGIN;
const devPort = Number(process.env.OPEN_COMPUTE_DASHBOARD_DEV_PORT ?? 5173);

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
    port: devPort,
    strictPort: devApiOrigin !== undefined,
    ...(devApiOrigin === undefined
      ? {}
      : {
          proxy: {
            "/client/v4": {
              target: devApiOrigin,
              ws: true,
              changeOrigin: true,
            },
            "/operator/session": { target: devApiOrigin, changeOrigin: true },
          },
        }),
  },
});
