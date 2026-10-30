import { Container } from "@cloudflare/containers";
import { execLifecycle } from "./exec-lifecycle";
import { execUsersLifecycle } from "./exec-users-lifecycle";
import {
  fixtureErrorCode,
  fixtureFailureDiagnostic,
} from "./failure-diagnostic";
import { RawIdleContainer } from "./inactivity-lifecycle";
import { NamedImageContainer } from "./named-images";
import { networkLifecycle } from "./network-lifecycle";

export { ContainerProxy } from "@cloudflare/containers";
export { RawIdleContainer, NamedImageContainer };

interface Env {
  CODE_REVISION: string;
  OBJECTS: DurableObjectNamespace<FixtureContainer>;
  IDLE_OBJECTS: DurableObjectNamespace<RawIdleContainer>;
  NAMED_OBJECTS: DurableObjectNamespace<NamedImageContainer>;
}

interface FixtureDiagnostic {
  phase: string;
  output?: { exitCode: number; stdoutBytes: number; stderrBytes: number };
}

async function fixtureFailure(
  error: unknown,
  diagnostic: FixtureDiagnostic,
): Promise<Response> {
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

export class FixtureContainer extends Container<Env> {
  static {
    this.outboundByHost = {
      "p23-specific.invalid": async () => new Response("http-specific-initial"),
      "p23-egress.invalid": async (request: Request, env: Env) => {
        if (new URL(request.url).pathname === "/https-probe") {
          if (
            new URL(request.url).protocol !== "https:" ||
            request.method !== "POST" ||
            request.headers.get("X-P23-Request") !== "tls-header" ||
            (await request.text()) !== "tls-body"
          )
            throw new Error("HTTPS interception request must match");
          return new Response(`https:${env.CODE_REVISION}:tls-body`, {
            status: 201,
            headers: { "X-P23-Reply": "tls-ok", Connection: "close" },
          });
        }
        return new Response(env.CODE_REVISION, {
          headers: {
            "X-Fixture-Code-Present":
              typeof env.CODE_REVISION === "string" ? "1" : "0",
          },
        });
      },
    };
  }

  defaultPort = 8080;
  sleepAfter = "10m";
  enableInternet = false;
  interceptHttps = true;
  allowedHosts = ["p23-egress.invalid", "p23-specific.invalid"];
  envVars = { PRIVATE_STARTUP: "p23-owned-startup" };
  labels = { purpose: "containers-fixture" };

  constructor(ctx: ConstructorParameters<typeof Container<Env>>[0], env: Env) {
    // Diagnostic writes use this constructor's existing public storage authority.
    const phase = (value: string, code: string | null = null) => {
      try {
        ctx.storage.sql.exec(
          "CREATE TABLE IF NOT EXISTS fixture_diagnostic_phase (phase TEXT NOT NULL, native_error_code TEXT)",
        );
        ctx.storage.sql.exec(
          "INSERT INTO fixture_diagnostic_phase (phase, native_error_code) VALUES (?, ?)",
          value,
          code,
        );
      } catch {
        // A diagnostic write must never replace the SDK's original error.
      }
    };
    phase("BEFORE_SDK_SUPER");
    try {
      super(ctx, env);
    } catch (error) {
      phase("SDK_SUPER_FAILED", fixtureErrorCode(error));
      throw error;
    }
    phase("SDK_SUPER_RETURNED");
  }

  async fetch(request: Request): Promise<Response> {
    const diagnostic: FixtureDiagnostic = { phase: "handler-enter" };
    try {
      return await this.fixtureFetch(request, diagnostic);
    } catch (error) {
      return fixtureFailure(error, diagnostic);
    }
  }

  private async fixtureFetch(
    request: Request,
    diagnostic: FixtureDiagnostic,
  ): Promise<Response> {
    for (const [name, marker] of [
      ["network-http-specific", "http-specific"],
      ["network-http-fallback", "http-fallback"],
    ] as const) {
      if (this.ctx.id.equals(this.env.OBJECTS.idFromName(name)))
        return new Response(marker, { headers: { Connection: "close" } });
    }
    diagnostic.phase = "native-attachment";
    const native = this.ctx.container;
    if (native === undefined)
      throw new Error("native container attachment is required");
    const path = new URL(request.url).pathname;
    if (
      path === "/exec-lifecycle" &&
      new URL(request.url).searchParams.get("object") !== "exec-lifecycle"
    )
      return new Response("independent exec object is required", {
        status: 400,
      });
    diagnostic.phase = "visits-initialize";
    this.ctx.storage.sql.exec(
      "CREATE TABLE IF NOT EXISTS visits (count INTEGER NOT NULL)",
    );
    this.ctx.storage.sql.exec(
      "INSERT INTO visits SELECT 0 WHERE NOT EXISTS (SELECT 1 FROM visits)",
    );
    if (path === "/stop") {
      await native.destroy();
      return Response.json({ running: native.running });
    }
    if (path === "/state") {
      return Response.json({
        running: native.running,
        visits: this.ctx.storage.sql
          .exec<{ count: number }>("SELECT count FROM visits")
          .one().count,
        code: this.env.CODE_REVISION,
      });
    }
    if (path === "/inspection") {
      const info = await native.inspect();
      return Response.json({
        running: native.running,
        info,
        images: native.images,
      });
    }
    diagnostic.phase = "sdk-start-ports";
    await this.startAndWaitForPorts({
      ports: 8080,
      cancellationOptions: { portReadyTimeoutMS: 20000, waitInterval: 50 },
    });
    if (path === "/exec-lifecycle") return execLifecycle(native);
    if (path === "/exec-users") return execUsersLifecycle(native, false);
    if (path === "/network-lifecycle")
      return networkLifecycle(
        native,
        this.env.CODE_REVISION,
        this.env.OBJECTS.get(
          this.env.OBJECTS.idFromName("network-http-specific"),
        ),
        this.env.OBJECTS.get(
          this.env.OBJECTS.idFromName("network-http-fallback"),
        ),
      );
    if (path === "/cancel") {
      const controller = new AbortController();
      const process = await native.exec(
        ["python3", "-c", "import time; time.sleep(30)"],
        {
          signal: controller.signal,
        },
      );
      controller.abort();
      // Abort after exec() sends SIGKILL; output() returns the process exit code.
      const abortedOutput = await process.output();
      const preAborted = new AbortController();
      preAborted.abort();
      let preAbortName: string | null = null;
      try {
        await native.exec(["/bin/sh", "-c", "printf unexpected"], {
          signal: preAborted.signal,
        });
      } catch (error) {
        if (!(error instanceof DOMException) || error.name !== "AbortError") {
          throw error;
        }
        preAbortName = error.name;
      }
      const probe = await native.exec(["/bin/sh", "-c", "printf alive"]);
      const output = await probe.output();
      return Response.json({
        exitCode: abortedOutput.exitCode,
        preAbortName,
        probe: new TextDecoder().decode(output.stdout),
      });
    }
    if (path === "/streams") {
      const process = await native.exec(
        ["/bin/sh", "-c", "cat; printf stderr >&2; exit 7"],
        { stdin: "pipe" },
      );
      if (process.stdin === null)
        throw new Error("exec stdin pipe is required");
      const writer = process.stdin.getWriter();
      await writer.write(new TextEncoder().encode("streamed-input"));
      await writer.close();
      const output = await process.output();
      const pty = await native.exec(
        [
          "python3",
          "-c",
          [
            "import os,signal,sys,termios",
            "signal.pthread_sigmask(signal.SIG_BLOCK,{signal.SIGWINCH})",
            "settings=termios.tcgetattr(0)",
            "settings[3] &= ~termios.ECHO",
            "termios.tcsetattr(0,termios.TCSANOW,settings)",
            "size=os.get_terminal_size(0)",
            "assert (size.columns,size.lines)==(80,24)",
            "print('initial:80x24',flush=True)",
            "assert sys.stdin.buffer.readline()==b'resize\\n'",
            "signal.sigwait({signal.SIGWINCH})",
            "size=os.get_terminal_size(0)",
            "assert (size.columns,size.lines)==(100,30)",
            "os.write(1,b'resized:100x30\\n')",
            "os.write(2,b'pty-stderr\\n')",
          ].join("\n"),
        ],
        { stdin: "pipe", pty: { cols: 80, rows: 24 } },
      );
      if (pty.stdin == null || pty.stdout == null)
        throw new Error("PTY stdin and stdout are required");
      const ptyStderr: unknown = Reflect.get(pty, "stderr");
      if (ptyStderr != null)
        throw new Error("PTY stderr must be merged into stdout");
      const ptyWriter = pty.stdin.getWriter();
      const ptyReader = pty.stdout.getReader();
      const ptyDecoder = new TextDecoder();
      let ptyInitial = "";
      let ptyStdout = "";
      let ptyExitCode: number;
      let ptyFinished = false;
      try {
        while (!ptyInitial.includes("\n")) {
          const chunk = await ptyReader.read();
          if (chunk.done) throw new Error("PTY initial size was not reported");
          ptyInitial += ptyDecoder.decode(chunk.value, { stream: true });
          if (ptyInitial.length > 128)
            throw new Error("PTY initial size response exceeded its bound");
        }
        if (ptyInitial !== "initial:80x24\r\n")
          throw new Error("PTY initial dimensions must be 80x24");
        pty.resize(100, 30);
        await ptyWriter.write(new TextEncoder().encode("resize\n"));
        for (;;) {
          const chunk = await ptyReader.read();
          if (chunk.done) break;
          ptyStdout += ptyDecoder.decode(chunk.value, { stream: true });
          if (ptyStdout.length > 128)
            throw new Error("PTY resized response exceeded its bound");
        }
        ptyStdout += ptyDecoder.decode();
        ptyExitCode = await pty.exitCode;
        if (
          !pty.isPty ||
          ptyStdout !== "resized:100x30\r\npty-stderr\r\n" ||
          ptyExitCode !== 0
        )
          throw new Error("PTY resize and merged stderr must complete exactly");
        ptyFinished = true;
      } finally {
        ptyWriter.releaseLock();
        ptyReader.releaseLock();
        if (!ptyFinished) pty.kill(9);
      }
      const ptyEnvironment = await native.exec(
        [
          "python3",
          "-c",
          "import os; assert all(name not in os.environ for name in ('PRIVATE_STARTUP', 'IMAGE_REVISION', 'HOSTNAME', 'TERM')); assert os.environ.get('PATH'); print('pty-env-ok')",
        ],
        { pty: { cols: 80, rows: 24 } },
      );
      const ptyEnvironmentOutput = await ptyEnvironment.output();
      return Response.json({
        stdout: new TextDecoder().decode(output.stdout),
        stderr: new TextDecoder().decode(output.stderr),
        exitCode: output.exitCode,
        pty: pty.isPty,
        ptyInitial,
        ptyStdout,
        ptyStderrAbsent: ptyStderr == null,
        ptyExitCode,
        ptyEnvironment: new TextDecoder()
          .decode(ptyEnvironmentOutput.stdout)
          .trim(),
        ptyEnvironmentExitCode: ptyEnvironmentOutput.exitCode,
      });
    }
    diagnostic.phase = "main-http";
    const main = await native.getTcpPort(8080).fetch("http://container/");
    diagnostic.phase = "default-exec";
    const process = await native.exec([
      "python3",
      "-c",
      "import os; assert all(name not in os.environ for name in ('PRIVATE_STARTUP', 'IMAGE_REVISION', 'HOSTNAME', 'TERM')); assert os.environ.get('PATH'); print('exec-ok', end='')",
    ]);
    diagnostic.phase = "default-output";
    const output = await process.output();
    diagnostic.output = {
      exitCode: output.exitCode,
      stdoutBytes: output.stdout.byteLength,
      stderrBytes: output.stderr.byteLength,
    };
    diagnostic.phase = "override-exec";
    const overridden = await native.exec(
      [
        "/bin/sh",
        "-c",
        'test "$PRIVATE_STARTUP" = exec-startup && test "$IMAGE_REVISION" = exec-image && test "$HOSTNAME" = exec-host && test "$TERM" = exec-term && test "$HOME" = /exec-home && test "$PATH" = exec-path && printf explicit-ok',
      ],
      {
        env: {
          PRIVATE_STARTUP: "exec-startup",
          IMAGE_REVISION: "exec-image",
          HOSTNAME: "exec-host",
          TERM: "exec-term",
          HOME: "/exec-home",
          PATH: "exec-path",
        },
      },
    );
    diagnostic.phase = "override-output";
    const overriddenOutput = await overridden.output();
    diagnostic.output = {
      exitCode: overriddenOutput.exitCode,
      stdoutBytes: overriddenOutput.stdout.byteLength,
      stderrBytes: overriddenOutput.stderr.byteLength,
    };
    diagnostic.phase = "egress-exec";
    const egress = await native.exec([
      "python3",
      "-c",
      "import http.client, sys; connection = http.client.HTTPConnection('203.0.113.1', 80, timeout=10); connection.request('GET', '/', headers={'Host': 'p23-egress.invalid'}); response = connection.getresponse(); payload = response.read(); print('egress-status=' + str(response.status) + ';code-present=' + response.getheader('X-Fixture-Code-Present', 'missing') + ';body-bytes=' + str(len(payload)), file=sys.stderr); print(payload.decode()); connection.close(); raise SystemExit(0 if response.status == 200 else 1)",
    ]);
    diagnostic.phase = "egress-output";
    const egressOutput = await egress.output();
    diagnostic.output = {
      exitCode: egressOutput.exitCode,
      stdoutBytes: egressOutput.stdout.byteLength,
      stderrBytes: egressOutput.stderr.byteLength,
    };
    const egressStatus =
      /^egress-status=(\d{3});code-present=([01]|missing);body-bytes=(\d+)\n$/.exec(
        new TextDecoder().decode(egressOutput.stderr),
      );
    this.ctx.storage.sql.exec(
      "CREATE TABLE IF NOT EXISTS fixture_egress_diagnostic (status INTEGER, code_present TEXT, body_bytes INTEGER, stdout_bytes INTEGER NOT NULL, stderr_bytes INTEGER NOT NULL, exit_code INTEGER NOT NULL)",
    );
    this.ctx.storage.sql.exec(
      "INSERT INTO fixture_egress_diagnostic VALUES (?, ?, ?, ?, ?, ?)",
      egressStatus === null ? null : Number(egressStatus[1]),
      egressStatus === null ? null : egressStatus[2],
      egressStatus === null ? null : Number(egressStatus[3]),
      egressOutput.stdout.byteLength,
      egressOutput.stderr.byteLength,
      egressOutput.exitCode,
    );
    diagnostic.phase = "visits-increment";
    this.ctx.storage.sql.exec("UPDATE visits SET count=count+1");
    return Response.json({
      image: (await main.text()).trim(),
      code: this.env.CODE_REVISION,
      visits: this.ctx.storage.sql
        .exec<{ count: number }>("SELECT count FROM visits")
        .one().count,
      running: native.running,
      exec: new TextDecoder().decode(output.stdout),
      exitCode: output.exitCode,
      execStderrBytes: output.stderr.byteLength,
      execStderrHasAssertion: new TextDecoder()
        .decode(output.stderr)
        .includes("AssertionError"),
      execOverride: new TextDecoder().decode(overriddenOutput.stdout),
      execOverrideExitCode: overriddenOutput.exitCode,
      egress: new TextDecoder().decode(egressOutput.stdout).trim(),
      egressExitCode: egressOutput.exitCode,
      envKeys: Object.keys(this.env).sort(),
    });
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const object = new URL(request.url).searchParams.get("object") ?? "primary";
    if (!/^[a-z0-9-]{1,40}$/.test(object))
      return Promise.resolve(new Response("invalid object", { status: 400 }));
    if (new URL(request.url).pathname.startsWith("/named/")) {
      try {
        return await env.NAMED_OBJECTS.getByName(object).fetch(request);
      } catch (error) {
        return fixtureFailure(error, {
          phase: "named-object-dispatch-or-construction",
        });
      }
    }
    const namespace = new URL(request.url).pathname.startsWith("/inactivity/")
      ? env.IDLE_OBJECTS
      : env.OBJECTS;
    const diagnostic: FixtureDiagnostic = { phase: "object-get-by-name" };
    try {
      const stub = namespace.getByName(object);
      diagnostic.phase = "object-fetch-or-construction";
      return await stub.fetch(request);
    } catch (error) {
      return fixtureFailure(error, diagnostic);
    }
  },
} satisfies ExportedHandler<Env>;
