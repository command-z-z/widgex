import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

export default defineConfig({
  // Relative asset paths so the build loads under the widgex:// custom protocol.
  base: "./",
  plugins: [solid()],
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: false,
  },
  test: {
    environment: "node",
  },
});
