import { DurableObject } from "cloudflare:workers";
import { directoryAction } from "./directory-snapshots";
import { execOptionsLifecycle } from "./exec-options-lifecycle";
import { execUsersLifecycle } from "./exec-users-lifecycle";
import {
  fixtureErrorCode,
  fixtureFailureDiagnostic,
  readinessDiagnostic,
} from "./failure-diagnostic";
import { signalsLifecycle } from "./signals-lifecycle";

type NativeContainer = NonNullable<DurableObjectState["container"]>;

async function ready(
  native: NativeContainer,
  diagnostic: NamedDiagnostic,
): Promise<Response> {
  const deadline = Date.now() + 20000;
  let attempts = 0;
  let lastStatus: number | null = null;
  let lastError: unknown;
  while (Date.now() < deadline) {
    attempts++;
    lastStatus = null;
    lastError = undefined;
    try {
      const response = await native.getTcpPort(8080).fetch(
        new Request("http://container/", {
          signal: AbortSignal.timeout(1000),
        }),
      );
      lastStatus = response.status;
      if (response.ok) return response;
      await response.body?.cancel();
    } catch (error) {
      lastError = error;
      // The main process can start before its HTTP listener.
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  diagnostic.readiness = await readinessDiagnostic(
    native,
    attempts,
    lastStatus,
    lastError,
  );
  throw new Error("named container HTTP readiness exceeded its bound");
}

interface NamedDiagnostic {
  phase: string;
  readiness?: Awaited<ReturnType<typeof readinessDiagnostic>>;
  output?: {
    exitCode: number;
    stdoutBytes: number;
    stderrBytes: number;
    stderrCode: string | null;
  };
}

export class NamedImageContainer extends DurableObject {
  async fetch(request: Request): Promise<Response> {
    const diagnostic: NamedDiagnostic = { phase: "named-handler-enter" };
    try {
      return await this.namedFetch(request, diagnostic);
    } catch (error) {
      return Response.json(
        {
          ok: false,
          diagnostic: "controlled-fixture",
          ...diagnostic,
          ...(await fixtureFailureDiagnostic(error)),
        },
        { status: 500 },
      );
    }
  }

  private async namedFetch(
    request: Request,
    diagnostic: NamedDiagnostic,
  ): Promise<Response> {
    diagnostic.phase = "named-native-attachment";
    const native = this.ctx.container;
    if (native === undefined)
      throw new Error("native container attachment is required");
    const url = new URL(request.url);
    const path = url.pathname;
    diagnostic.phase = "named-images";
    const images = native.images;
    const base = images.base;
    if (base === undefined || images.alternate === undefined)
      throw new Error("both named images are required");
    if (path.startsWith("/named/directory/")) {
      diagnostic.phase = "directory-action";
      return directoryAction(native, base, request);
    }
    if (path === "/named/images") {
      const first = native.images;
      const second = native.images;
      first.base = "changed-local-copy";
      delete first.alternate;
      return Response.json({
        images: native.images,
        independent:
          first !== second &&
          second.base === base &&
          second.alternate === images.alternate &&
          native.images.base === base &&
          native.images.alternate === images.alternate,
      });
    }
    if (path === "/named/inspection") {
      const info = await native.inspect();
      return Response.json({
        running: native.running,
        info,
        images,
      });
    }
    if (path === "/named/surface")
      return Response.json({
        snapshotContainer: typeof native.snapshotContainer,
        snapshotDirectory: typeof Reflect.get(native, "snapshotDirectory"),
      });
    if (path === "/named/stop") {
      await native.destroy();
      return Response.json({ running: native.running });
    }
    if (path === "/named/missing") {
      try {
        native.start({ enableInternet: false, instance: "lite" });
        const probe = await native.exec(["/bin/sh", "-c", "printf unexpected"]);
        await probe.output();
        return Response.json({ denied: false });
      } catch {
        await native.monitor().catch(() => undefined);
        const info = await native.inspect();
        return Response.json({
          denied: true,
          running: native.running,
          info,
        });
      }
    }
    if (path === "/named/signals-lifecycle")
      return signalsLifecycle(native, base);
    if (path === "/named/restore-image") {
      const image = url.searchParams.get("image");
      if (
        image === null ||
        !/^[a-z0-9-]{1,64}@sha256:[0-9a-f]{64}$/.test(image)
      )
        return new Response("invalid image", { status: 400 });
      try {
        native.start({ image, enableInternet: false, instance: "lite" });
        const probe = await native.exec(["/bin/sh", "-c", "printf unexpected"]);
        await probe.output();
        return Response.json({ denied: false });
      } catch {
        return Response.json({ denied: true });
      }
    }
    const options = {
      enableInternet: false,
      instance: "lite",
      env: { PRIVATE_STARTUP: "p23-owned-startup" },
      labels: { purpose: "containers-fixture" },
    } satisfies ContainerStartupOptions;
    if (
      ![
        "/named/start",
        "/named/exec-options",
        "/named/exec-users",
        "/named/snapshot-write",
        "/named/snapshot",
        "/named/snapshot-default",
        "/named/snapshot-state",
        "/named/restore",
      ].includes(path)
    )
      return new Response("unknown named image action", { status: 404 });
    diagnostic.phase = "named-start";
    if (path === "/named/restore") {
      const id = url.searchParams.get("id");
      if (
        id === null ||
        !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(
          id,
        )
      )
        return new Response("invalid snapshot", { status: 400 });
      native.start({ ...options, containerSnapshot: { id } });
    } else if (!native.running) {
      const alias =
        path === "/named/start" ? url.searchParams.get("alias") : "base";
      if (alias !== "base" && alias !== "alternate")
        return new Response("invalid image alias", { status: 400 });
      const image = images[alias];
      if (image === undefined) throw new Error("named image is required");
      if (path === "/named/start") {
        const instance = url.searchParams.get("instance");
        if (instance !== null && instance !== "lite")
          return new Response("invalid instance", { status: 400 });
        native.start({
          image,
          enableInternet: false,
          labels: options.labels,
          ...(instance === "lite"
            ? { instance: "lite" }
            : alias === "alternate"
              ? {
                  instance: { vcpu: 0.03125, memoryMib: 128, diskMb: 1000 },
                }
              : {}),
        });
      } else native.start({ ...options, image });
    }
    diagnostic.phase = "named-readiness";
    const response = await ready(native, diagnostic);
    if (path === "/named/start")
      return Response.json({
        image: (await response.text()).trim(),
        info: await native.inspect(),
        running: native.running,
      });
    diagnostic.phase = "named-ready-body-cancel";
    await response.body?.cancel();
    if (path === "/named/exec-users") return execUsersLifecycle(native, true);
    if (path === "/named/exec-options") {
      try {
        return await execOptionsLifecycle(native, diagnostic);
      } finally {
        await native.destroy();
      }
    }
    if (path === "/named/snapshot-write") {
      diagnostic.phase = "snapshot-write-value";
      const value = url.searchParams.get("value");
      if (value !== "saved" && value !== "mutated")
        return new Response("invalid value", { status: 400 });
      diagnostic.phase = "snapshot-write-exec";
      const process = await native.exec(
        [
          "python3",
          "-c",
          "import os,sys; paths=('/tmp/p23-snapshot-state','/run/p23-snapshot-volatile'); [(open(path,'w').write(sys.argv[1]),os.chmod(path,0o640)) for path in paths]",
          value,
        ],
        { user: "0:0", cwd: "/" },
      );
      diagnostic.phase = "snapshot-write-output";
      const output = await process.output();
      const stderr = new TextDecoder().decode(output.stderr.slice(0, 2048));
      diagnostic.output = {
        exitCode: output.exitCode,
        stdoutBytes: output.stdout.byteLength,
        stderrBytes: output.stderr.byteLength,
        stderrCode:
          [
            { prefix: "PermissionError:", code: "PYTHON_PERMISSION_ERROR" },
            { prefix: "FileNotFoundError:", code: "PYTHON_FILE_NOT_FOUND" },
            {
              prefix: "Read-only file system",
              code: "PYTHON_READ_ONLY_FILESYSTEM",
            },
            { prefix: "No space left on device", code: "PYTHON_NO_SPACE" },
          ].find(({ prefix }) => stderr.includes(prefix))?.code ?? null,
      };
      diagnostic.phase = "snapshot-write-exit";
      if (output.exitCode !== 0)
        throw new Error("snapshot fixture write failed");
      return Response.json({ written: value });
    }
    if (path === "/named/snapshot" || path === "/named/snapshot-default") {
      this.ctx.storage.sql.exec(
        "CREATE TABLE IF NOT EXISTS snapshot_diagnostic_phase (phase TEXT NOT NULL, native_error_code TEXT, response_bytes INTEGER)",
      );
      // Return headers before capture; the bounded body still waits for the completed snapshot.
      return new Response(
        new ReadableStream<Uint8Array>({
          start: async (controller) => {
            this.ctx.storage.sql.exec(
              "INSERT INTO snapshot_diagnostic_phase VALUES (?, NULL, NULL)",
              "CAPTURE_REQUESTED",
            );
            try {
              const snapshot =
                path === "/named/snapshot-default"
                  ? await native.snapshotContainer()
                  : await native.snapshotContainer({ name: "fixture-saved" });
              const body = new TextEncoder().encode(JSON.stringify(snapshot));
              this.ctx.storage.sql.exec(
                "INSERT INTO snapshot_diagnostic_phase VALUES (?, NULL, ?)",
                "CAPTURE_RETURNED",
                body.byteLength,
              );
              controller.enqueue(body);
              controller.close();
            } catch (error) {
              this.ctx.storage.sql.exec(
                "INSERT INTO snapshot_diagnostic_phase VALUES (?, ?, NULL)",
                "CAPTURE_REJECTED",
                fixtureErrorCode(error),
              );
              controller.error(new Error("snapshot fixture capture failed"));
            }
          },
        }),
        { headers: { "Content-Type": "application/json" } },
      );
    }
    if (path === "/named/snapshot-state" || path === "/named/restore") {
      const process = await native.exec(
        [
          "python3",
          "-c",
          "import json,os,stat; path='/tmp/p23-snapshot-state'; print(json.dumps({'value':open(path).read() if os.path.exists(path) else None,'mode':stat.S_IMODE(os.stat(path).st_mode) if os.path.exists(path) else None,'volatile':os.path.exists('/run/p23-snapshot-volatile')}))",
        ],
        { user: "0:0", cwd: "/" },
      );
      const output = await process.output();
      if (output.exitCode !== 0)
        throw new Error("snapshot fixture read failed");
      return new Response(new TextDecoder().decode(output.stdout), {
        headers: { "Content-Type": "application/json" },
      });
    }
    return new Response("unknown named image action", { status: 404 });
  }
}
