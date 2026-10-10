import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { resolve } from "node:path";

// Three small pages, one per window.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  build: {
    target: "es2022",
    rollupOptions: {
      input: {
        pet: resolve(__dirname, "pet.html"),
        panel: resolve(__dirname, "panel.html"),
        settings: resolve(__dirname, "settings.html"),
      },
    },
  },
});
