import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: [
      // workerd built-in; tests inject their own connect() instead.
      {
        find: /^cloudflare:sockets$/,
        replacement: fileURLToPath(new URL("./test/stubs/cloudflare-sockets.ts", import.meta.url)),
      },
    ],
  },
  test: {
    environment: "node",
    include: ["test/**/*.test.ts"],
  },
});
