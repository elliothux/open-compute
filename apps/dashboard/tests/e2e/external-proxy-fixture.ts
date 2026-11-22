import { execFile, spawn } from "node:child_process";
import { once } from "node:events";
import { closeSync, openSync } from "node:fs";
import { appendFile, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { join, resolve } from "node:path";
import { promisify } from "node:util";
import { firefox } from "@playwright/test";
import { expect, test } from "./fixtures";

const run = promisify(execFile);
const root = resolve(import.meta.dirname, "../../../..");
const hostname = "dashboard-worker.example.test";

async function freePorts() {
  const listeners = Array.from({ length: 3 }, () => createServer());
  try {
    return (await Promise.all(
      listeners.map(async (server) => {
        server.listen(0, "127.0.0.1");
        await once(server, "listening");
        const address = server.address();
        if (!address || typeof address === "string")
          throw new Error("Expected a loopback TCP listener");
        return address.port;
      }),
    )) as [number, number, number];
  } finally {
    for (const server of listeners) server.close();
  }
}

function start(
  command: string,
  args: string[],
  env: NodeJS.ProcessEnv,
  log: string,
) {
  const fd = openSync(log, "w");
  try {
    const child = spawn(command, args, { env, stdio: ["ignore", fd, fd] });
    const exited = new Promise<void>((done) =>
      child.once("close", () => done()),
    );
    let error: Error | undefined;
    child.on("error", (cause) => {
      error = cause;
    });
    return {
      check() {
        if (error) throw error;
        if (child.exitCode !== null || child.signalCode !== null)
          throw new Error(`${command} exited during startup. See ${log}`);
      },
      async stop() {
        child.kill("SIGTERM");
        const timer = setTimeout(() => child.kill("SIGKILL"), 30_000);
        try {
          await exited;
        } finally {
          clearTimeout(timer);
        }
      },
    };
  } finally {
    closeSync(fd);
  }
}

export const externalProxyTest = test.extend<{
  externalProxy: { dashboard: string; origin: string; profile: string };
}>({
  externalProxy: async ({}, use, testInfo) => {
    const binary = process.env.OPEN_COMPUTE_OCD_BIN;
    const caddy = process.env.OPEN_COMPUTE_BUILD_CADDY;
    const certutil = process.env.OPEN_COMPUTE_DASHBOARD_E2E_CERTUTIL;
    if (!binary || !caddy || !certutil)
      throw new Error("Use apps/dashboard/scripts/run-e2e.sh --external-proxy");

    const temp = join(root, ".temp");
    await mkdir(temp, { recursive: true });
    const directory = await mkdtemp(join(temp, "ep-"));
    testInfo.annotations.push({
      type: "external proxy logs",
      description: directory,
    });
    const [port, ingressPort, tlsPort] = await freePorts();
    const dashboard = `http://127.0.0.1:${port}/operator/`;
    const origin = `https://${hostname}:${tlsPort}`;
    // ocd derives per-instance Unix socket paths from the scope directory.
    const scope = await mkdtemp("/tmp/ocd-pw-");
    try {
      const state = join(scope, "user");
      const env = {
        ...process.env,
        OPEN_COMPUTE_OCD_BIN: resolve(binary),
        OPEN_COMPUTE_DEV_PORT: String(port),
        OPEN_COMPUTE_DEV_OCD_ROOT: scope,
        OPEN_COMPUTE_DEV_STATE_DIR: directory,
        OPEN_COMPUTE_DEV_PRODUCTION_SCOPE: "0",
      };
      const devTest = join(root, "scripts/dev-test.sh");
      // Provision the regular lifecycle instance, then authorize the hostname before restart.
      await run(devTest, ["smoke"], { env, timeout: 120_000 });
      await appendFile(
        join(state, "ocd.toml"),
        `\n[external_ingress]\nbind = "127.0.0.1:${ingressPort}"\ntrusted_proxies = ["127.0.0.1/32"]\n`,
      );
      await appendFile(
        join(state, "instances/dev/compute.toml"),
        `\n[external_ingress]\nhostnames = ["${hostname}"]\n`,
      );

      const config = join(directory, "Caddyfile");
      const storage = join(scope, "caddy");
      const ca = join(storage, "pki/authorities/local/root.crt");
      await writeFile(
        config,
        `{
  admin off
  skip_install_trust
  auto_https disable_redirects
  storage file_system "${storage}"
}
${origin} {
  bind 127.0.0.1
  tls internal
  reverse_proxy http://127.0.0.1:${ingressPort}
}
`,
      );
      const daemon = start(
        devTest,
        ["run"],
        env,
        join(directory, "dev-test.log"),
      );
      try {
        await expect
          .poll(
            async () => {
              daemon.check();
              const response = await fetch(dashboard, {
                signal: AbortSignal.timeout(1_000),
              }).catch(() => null);
              return response?.status;
            },
            { timeout: 75_000 },
          )
          .toBe(200);
        const proxy = start(
          resolve(caddy),
          ["run", "--adapter", "caddyfile", "--config", config],
          { ...process.env, XDG_CONFIG_HOME: scope, XDG_DATA_HOME: scope },
          join(directory, "caddy.log"),
        );
        try {
          // A verified 404 proves TLS and ingress are ready before any Worker is bound.
          await expect
            .poll(
              async () => {
                proxy.check();
                const response = await run("curl", [
                  "--noproxy",
                  "*",
                  "--silent",
                  "--show-error",
                  "--max-time",
                  "1",
                  "--cacert",
                  ca,
                  "--resolve",
                  `${hostname}:${tlsPort}:127.0.0.1`,
                  "--output",
                  "/dev/null",
                  "--write-out",
                  "%{http_code}",
                  origin,
                ]).catch(() => null);
                return response?.stdout;
              },
              { timeout: 15_000 },
            )
            .toBe("404");
          const profile = join(scope, "firefox");
          await mkdir(profile);
          const database = `sql:${profile}`;
          await run(certutil, ["-N", "-d", database, "--empty-password"], {
            timeout: 10_000,
          });
          await run(
            certutil,
            [
              "-A",
              "-d",
              database,
              "-n",
              "Dashboard test CA",
              "-t",
              "C,,",
              "-i",
              ca,
            ],
            { timeout: 10_000 },
          );
          await use({ dashboard, origin, profile });
        } finally {
          await proxy.stop();
        }
      } finally {
        await daemon.stop();
      }
    } finally {
      await rm(scope, { recursive: true, force: true });
    }
  },
  context: async ({ externalProxy }, use) => {
    const context = await firefox.launchPersistentContext(
      externalProxy.profile,
      {
        baseURL: externalProxy.dashboard,
        firefoxUserPrefs: {
          "network.dns.localDomains": hostname,
          "network.proxy.type": 0,
        },
      },
    );
    try {
      await use(context);
    } finally {
      await context.close();
    }
  },
});
