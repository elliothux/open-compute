import test from "node:test";
import assert from "node:assert/strict";
import { importRuntime } from "../compiled-runtime.mjs";

const { routeDefaultHttp } = await importRuntime("assets/router.ts");

function snapshot(overrides = {}) {
  return {
    contentKind: "worker",
    assets: {
      manifest: {
        schemaVersion: 1,
        entries: [
          { path: "/404.html", sha256: "01".repeat(32), size: 1, contentType: "text/html" },
          { path: "/index.html", sha256: "02".repeat(32), size: 2, contentType: "text/html" },
          { path: "/static/app.js", sha256: "03".repeat(32), size: 3, contentType: "text/javascript" },
        ],
      },
      routing: {
        schemaVersion: 1,
        runWorkerFirst: false,
        htmlHandling: "auto-trailing-slash",
        notFoundHandling: "single-page-application",
        headers: [],
        redirects: [],
      },
    },
    ...overrides,
  };
}

function request(path, init) {
  return new Request(`https://example.com${path}`, init);
}

test("default router selects one path without invoking a speculative asset fetch", () => {
  assert.equal(routeDefaultHttp(snapshot(), request("/static/app.js")), "asset");
  assert.equal(routeDefaultHttp(snapshot(), request("/api/data")), "worker");
  assert.equal(routeDefaultHttp(snapshot({ contentKind: "assets_only" }), request("/api/data")), "asset");
  assert.equal(routeDefaultHttp(snapshot(), request("/static/app.js", { method: "POST" })), "worker");
});

test("worker-first exclusions, pinned navigation, redirects, and encodings are fixed", () => {
  const ruled = snapshot();
  ruled.assets.routing.runWorkerFirst = ["/api/*", "!/api/docs/*"];
  assert.equal(routeDefaultHttp(ruled, request("/api/value")), "worker");
  assert.equal(routeDefaultHttp(ruled, request("/api/docs/value")), "asset");

  assert.equal(routeDefaultHttp(snapshot(), request("/missing", { headers: { "sec-fetch-mode": "navigate" } })), "asset");
  const noFallback = snapshot();
  noFallback.assets.routing.notFoundHandling = "none";
  assert.equal(routeDefaultHttp(noFallback, request("/missing", { headers: { "sec-fetch-mode": "navigate" } })), "worker");

  const redirected = snapshot();
  redirected.assets.routing.redirects = [{ from: "/old/:name", to: "/new/:name", status: 308 }];
  assert.equal(routeDefaultHttp(redirected, request("/old/page")), "asset");
  assert.equal(routeDefaultHttp(snapshot(), request("/static%2Fapp.js")), "worker");
});
