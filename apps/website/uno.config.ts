import { presetStarlightIcons } from "starlight-plugin-icons/uno";
import { defineConfig, presetIcons } from "unocss";
import { docsNavigationIconClasses } from "./src/docs-topics";

export default defineConfig({
  presets: [
    presetStarlightIcons(),
    presetIcons({
      autoInstall: false,
    }),
  ],
  safelist: docsNavigationIconClasses,
});
