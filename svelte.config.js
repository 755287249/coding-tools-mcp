// Tauri doesn't have a Node.js server to do proper SSR
// so we use adapter-static with a fallback to index.html to put the site in SPA mode
// See: https://svelte.dev/docs/kit/single-page-apps
// See: https://v2.tauri.app/start/frontend/sveltekit/ for more info
import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

const base = process.env.CTMCP_UI_BASE ?? "";
const outDir = process.env.CTMCP_UI_OUT ?? "build";

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    output: { bundleStrategy: "single" },
    adapter: adapter({
      fallback: "index.html",
      pages: outDir,
      assets: outDir,
    }),
    paths: {
      base,
    },
    alias: {
      "$lib/backend/host":
        process.env.CTMCP_UI_HOST === "node"
          ? "src/lib/backend/host-node.ts"
          : "src/lib/backend/host-desktop.ts",
    },
  },
};

export default config;
