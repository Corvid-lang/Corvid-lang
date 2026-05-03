// Phase 23-F headless-Chromium configuration. The browser CI test
// loads `web/index.html` against a static file server that serves the
// repository-relative `examples/wasm_browser_demo/` directory so the
// in-page import of `../target/wasm/refund_gate.js` resolves.

import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  timeout: 60_000,
  reporter: [["list"]],
  use: {
    baseURL: "http://127.0.0.1:8765",
    headless: true,
    ignoreHTTPSErrors: true,
    viewport: { width: 1280, height: 720 },
  },
  webServer: {
    // Serve the parent demo directory so `web/index.html` can import
    // `../target/wasm/refund_gate.js`.
    command: "node static-server.mjs",
    url: "http://127.0.0.1:8765/web/",
    reuseExistingServer: false,
    stdout: "pipe",
    stderr: "pipe",
    timeout: 30_000,
  },
});
