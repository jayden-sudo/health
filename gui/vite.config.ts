// Dev / Playwright: the UI talks to a real `health serve` through this proxy (same origin, so no CORS).
// The proxy drops the browser's Origin header (serve refuses any) and adds the owner token from HEALTH_SERVE.
import react from "@vitejs/plugin-react";
import { readFileSync } from "node:fs";
import { defineConfig } from "vite";

function serve(): { port: number; token: string } | null {
  const file = process.env.HEALTH_SERVE; // path to the JSON line `health serve` printed
  return file ? JSON.parse(readFileSync(file, "utf8")) : null;
}

export default defineConfig(() => {
  const s = serve();
  return {
    plugins: [react()],
    clearScreen: false,
    server: {
      port: 5173,
      strictPort: true,
      proxy: s
        ? {
            "/v1": {
              target: `http://127.0.0.1:${s.port}`,
              changeOrigin: true,
              // the token is added below, so only this dev page may use the proxy: a page from another origin
              // (a site open in the same browser) is turned away before anything reaches serve
              bypass: (req) => {
                const o = req.headers.origin;
                return o && o !== `http://${req.headers.host}` ? false : undefined;
              },
              configure: (proxy) => {
                proxy.on("proxyReq", (req) => {
                  req.removeHeader("origin");
                  req.removeHeader("referer");
                  req.setHeader("authorization", `Bearer ${s.token}`);
                });
              },
            },
          }
        : undefined,
    },
    build: { target: "es2022", outDir: "dist" },
  };
});
