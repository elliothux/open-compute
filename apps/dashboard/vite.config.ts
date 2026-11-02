import {
  cpSync,
  existsSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
} from "node:fs";
import { resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const stagedDist = resolve(import.meta.dirname, "../../.temp/dashboard-build");
const dist = resolve(import.meta.dirname, "dist");

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
    {
      name: "stable-dashboard-assets",
      apply: "build",
      writeBundle() {
        // Cargo watches files and directories, so leave identical assets in place.
        if (existsSync(dist)) {
          for (const name of readdirSync(dist, {
            recursive: true,
            encoding: "utf8",
          }).reverse()) {
            const source = resolve(stagedDist, name);
            const destination = resolve(dist, name);
            if (
              !existsSync(source) ||
              statSync(source).isDirectory() !==
                statSync(destination).isDirectory()
            ) {
              rmSync(destination, { recursive: true, force: true });
            }
          }
        }
        cpSync(stagedDist, dist, {
          recursive: true,
          force: true,
          filter: (source, destination) =>
            statSync(source).isDirectory() ||
            !existsSync(destination) ||
            !readFileSync(source).equals(readFileSync(destination)),
        });
      },
    },
  ],
  build: {
    sourcemap: false,
    outDir: stagedDist,
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
