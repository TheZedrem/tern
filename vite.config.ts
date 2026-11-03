import { gzipSync } from "node:zlib";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig, type Plugin } from "vite";
import solid from "vite-plugin-solid";

/** Entry-chunk budget from spec 0001. Raising it needs the owner's OK. */
const ENTRY_BUDGET_GZIP_BYTES = 150 * 1024;

/**
 * Fails the build when the entry JS chunk (everything `src/main.tsx` pulls in
 * statically) exceeds the gzip budget. This is the guard against boot-path rot.
 */
function entryBudget(limit: number): Plugin {
  return {
    name: "zed-term:entry-budget",
    apply: "build",
    generateBundle(_options, bundle) {
      for (const chunk of Object.values(bundle)) {
        if (chunk.type !== "chunk" || !chunk.isEntry) continue;
        const size = gzipSync(chunk.code).length;
        const kb = (n: number) => `${(n / 1024).toFixed(1)} KB`;
        if (size > limit) {
          this.error(
            `Entry chunk ${chunk.fileName} is ${kb(size)} gzipped, over the ${kb(limit)} budget (spec 0001). ` +
              "Move the new code behind import() instead of raising the budget.",
          );
        }
        this.info(`entry chunk ${chunk.fileName}: ${kb(size)} gzipped (budget ${kb(limit)})`);
      }
    },
  };
}

export default defineConfig({
  plugins: [solid(), tailwindcss(), entryBudget(ENTRY_BUDGET_GZIP_BYTES)],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  build: { target: "es2022" },
});
