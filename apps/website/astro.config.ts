import react from "@astrojs/react";
import starlight from "@astrojs/starlight";
import type { AstroIntegration } from "astro";
import { defineConfig } from "astro/config";
import starlightLinksValidator from "starlight-links-validator";
import starlightLlmsTxt from "starlight-llms-txt";
import starlightSidebarTopics from "starlight-sidebar-topics";
import starlightThemeBlack from "starlight-theme-black";
import UnoCSS from "unocss/astro";
import { docsSidebarTopicOptions, docsSidebarTopics } from "./src/docs-topics";

function docsSidebarComposition(): import("@astrojs/starlight/types").StarlightPlugin {
  return {
    name: "open-compute-docs-sidebar",
    hooks: {
      "config:setup": ({ config, updateConfig }) => {
        updateConfig({
          components: {
            ...config.components,
            Sidebar: "./src/components/docs-sidebar.astro",
          },
        });
      },
    },
  };
}

function githubApiDevServer(): AstroIntegration {
  return {
    name: "github-api-dev-server",
    hooks: {
      "astro:config:setup": ({ command, injectRoute }) => {
        if (command !== "dev") return;

        for (const pattern of ["/api/github-stars", "/api/releases"]) {
          injectRoute({
            entrypoint: "./src/dev-api-route.ts",
            pattern,
            prerender: false,
          });
        }
      },
    },
  };
}

export default defineConfig({
  site: "https://open-compute.dev",
  output: "static",
  trailingSlash: "always",
  integrations: [
    UnoCSS(),
    react(),
    githubApiDevServer(),
    starlight({
      title: "open-compute",
      description:
        "Install, develop for, and operate the open-compute single-node Cloudflare Workers-compatible platform.",
      logo: {
        dark: "../../share/brand/logo-text-white.svg",
        light: "../../share/brand/logo-text-black.svg",
        alt: "open-compute",
        replacesTitle: true,
      },
      components: {
        LanguageSelect: "./src/components/docs-language-select.astro",
        Search: "./src/components/docs-search.astro",
      },
      customCss: ["./src/docs-brand.css"],
      disable404Route: true,
      editLink: {
        baseUrl:
          "https://github.com/elliothux/open-compute/edit/main/apps/website/src/content/docs/docs/",
      },
      head: [
        {
          tag: "meta",
          attrs: {
            name: "algolia-site-verification",
            content: "7036571FA1C21978",
          },
        },
      ],
      lastUpdated: true,
      pagefind: false,
      plugins: [
        starlightThemeBlack({}),
        starlightSidebarTopics(docsSidebarTopics, docsSidebarTopicOptions),
        docsSidebarComposition(),
        starlightLlmsTxt({
          projectName: "open-compute",
          description:
            "A self-hosted, single-node Cloudflare Workers-compatible platform built around one `ocd` binary and a pinned workerd runtime.",
          details:
            "Use the current documentation as the source of truth. Inspect an existing installation before changing it, preserve configuration and instance data by default, and ask before privileged or destructive operations.",
          promote: [
            "index*",
            "get-started*",
            "develop/index*",
            "operate/index*",
          ],
        }),
        starlightLinksValidator(),
      ],
      routeMiddleware: "./src/starlight-route.ts",
      social: [
        {
          icon: "github",
          label: "GitHub",
          href: "https://github.com/elliothux/open-compute",
        },
      ],
    }),
  ],
  vite: {
    build: {
      rollupOptions: {
        external: ["satteri"],
      },
    },
    envPrefix: "PUBLIC_",
  },
});
