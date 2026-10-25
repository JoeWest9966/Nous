import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { VitePWA } from "vite-plugin-pwa";

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    VitePWA({
      registerType: "autoUpdate",
      includeAssets: ["icon.svg"],
      manifest: {
        name: "N",
        short_name: "N",
        description: "The Durable Agent Network panel.",
        display: "standalone",
        start_url: "/",
        theme_color: "#0b0d11",
        background_color: "#0b0d11",
        icons: [{ src: "icon.svg", sizes: "any", type: "image/svg+xml", purpose: "any" }],
      },
      workbox: { globPatterns: ["**/*.{js,css,html,svg,woff2}"], navigateFallback: "index.html" },
    }),
  ],
  // Own port, own host: this package never shares a server with the old panel.
  server: { host: "0.0.0.0", port: 4783, strictPort: true },
  preview: { host: "0.0.0.0", port: 4783 },
});
