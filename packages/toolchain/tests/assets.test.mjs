import assert from "node:assert/strict";
import { mkdtemp, mkdir, open, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { readAssetObject, scanAssets } from "../src/assets/scan.ts";
import { parseHeaders, parseRedirects } from "../src/assets/rules.ts";

const config = {
  directory: "dist",
  binding: "STATIC",
  runWorkerFirst: ["/api/*", "!/api/docs/*"],
  htmlHandling: "auto-trailing-slash",
  notFoundHandling: "404-page",
  publishSourceMaps: false,
};

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), "open-compute-assets-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, "dist", "static"), { recursive: true });
  return root;
}

test("scans immutable bytes into sorted encoded paths and parses control files", async t => {
  const root = await fixture(t);
  await writeFile(join(root, "dist", "index.html"), "<main>hello</main>");
  await writeFile(join(root, "dist", "static", "% [name].js"), "export default 1;");
  await writeFile(join(root, "dist", "static", "private.map"), "secret source");
  await writeFile(join(root, "dist", "_headers"), "/*\n  X-Frame-Options: DENY\n/static/*\n  Cache-Control: public, immutable\n");
  await writeFile(join(root, "dist", "_redirects"), "/old/:name /new/:name 308\n/rewrite /index.html 200\n");
  const scanned = await scanAssets(root, config);
  assert.deepEqual(scanned.manifest.entries.map(entry => entry.path), [
    "/index.html", "/static/%25%20%5Bname%5D.js",
  ]);
  assert.equal(scanned.routing.headers[0].operations[0].name, "x-frame-options");
  assert.equal(scanned.routing.redirects[1].status, 200);
  assert.equal(scanned.objects.size, 2);
  const source = scanned.objects.get(scanned.manifest.entries[0].sha256);
  assert.equal(new TextDecoder().decode(await readAssetObject(source)), "<main>hello</main>");
});

test("refuses symlinks, project roots, forbidden files, and post-scan changes", async t => {
  const root = await fixture(t);
  await writeFile(join(root, "outside.txt"), "outside");
  await symlink(join(root, "outside.txt"), join(root, "dist", "escape.txt"));
  await assert.rejects(scanAssets(root, config), /symbolic link/);
  await rm(join(root, "dist", "escape.txt"));
  await writeFile(join(root, "dist", ".env.production"), "PRIVATE=secret");
  await assert.rejects(scanAssets(root, config), /forbidden path/);
  await rm(join(root, "dist", ".env.production"));
  await writeFile(join(root, "dist", "index.html"), "first");
  const scanned = await scanAssets(root, config);
  const source = [...scanned.objects.values()][0];
  await writeFile(source.filename, "second");
  await assert.rejects(readAssetObject(source), /changed after manifest/);
  await assert.rejects(scanAssets(root, { ...config, directory: "." }), /dedicated directory/);
});

test("rule parsers reject malformed lines instead of silently dropping them", () => {
  assert.throws(() => parseHeaders("  X-Test: value"), /no rule pattern/);
  assert.throws(() => parseHeaders("/*\n  X-Test: one\n  X-Test: two"), /repeats/);
  assert.throws(() => parseRedirects("/from https://example.com 999"), /invalid/);
  assert.deepEqual(parseRedirects("/from /to"), [{ from: "/from", to: "/to", status: 302 }]);
});

test("ignore, Unicode, well-known paths, source-map policy, CRLF, and quotas are fixed", async t => {
  const root = await fixture(t);
  await mkdir(join(root, "dist", ".well-known"));
  await writeFile(join(root, "dist", ".assetsignore"), "ignored.*\r\n!ignored.keep\r\n");
  await writeFile(join(root, "dist", "ignored.txt"), "not published");
  await writeFile(join(root, "dist", "ignored.keep"), "published");
  await writeFile(join(root, "dist", "café.html"), "unicode");
  await writeFile(join(root, "dist", ".well-known", "security.txt"), "contact");
  await writeFile(join(root, "dist", "client.js.map"), "{}");
  await writeFile(join(root, "dist", "_headers"), "/*\r\n  X-Test: yes\r\n");
  const hidden = await scanAssets(root, config);
  assert.deepEqual(hidden.manifest.entries.map(entry => entry.path), [
    "/.well-known/security.txt", "/caf%C3%A9.html", "/ignored.keep",
  ]);
  assert.equal(hidden.routing.headers[0].operations[0].value, "yes");
  const visible = await scanAssets(root, { ...config, publishSourceMaps: true });
  assert.ok(visible.manifest.entries.some(entry => entry.path === "/client.js.map"));

  const large = await open(join(root, "dist", "too-large.bin"), "w");
  await large.truncate(25 * 1024 * 1024 + 1);
  await large.close();
  await assert.rejects(scanAssets(root, config), /bounded regular file/);
});
