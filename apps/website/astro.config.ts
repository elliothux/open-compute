import react from "@astrojs/react";
import starlight from "@astrojs/starlight";
import type { AstroIntegration } from "astro";
import { defineConfig } from "astro/config";

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
    react(),
    githubApiDevServer(),
    starlight({
      title: "open-compute",
      description: "open-compute developer documentation",
      components: {
        LanguageSelect: "./src/components/docs-language-select.astro",
      },
      customCss: ["./src/docs.css"],
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
    envPrefix: "PUBLIC_",
  },
});
