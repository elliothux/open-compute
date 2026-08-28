import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { isAbsolute } from "node:path";
import type { CompiledWorker } from "./build-worker.ts";

/** Canonical artifact returned by the platform's existing Rust encoder. */
export interface WorkerArtifact {
  readonly mainModule: string;
  readonly bytes: Uint8Array;
  readonly sha256: string;
}

/** Encode in a separate offline command; no platform config, S3, or workerd is opened. */
export function encodeWorker(worker: CompiledWorker, platformd: string): Promise<WorkerArtifact> {
  if (!isAbsolute(platformd)) throw new Error("platformd binary path must be absolute");
  const input = JSON.stringify({
    schemaVersion: 1,
    mainModule: worker.mainModule,
    modules: worker.modules.map(module => ({
      name: module.name, type: module.type, bytesBase64: Buffer.from(module.bytes).toString("base64"),
    })),
  });
  return new Promise((accept, reject) => {
    const child = execFile(platformd, ["worker", "bundle"], {
      encoding: null, maxBuffer: 17 * 1024 * 1024, timeout: 30_000, killSignal: "SIGKILL",
    }, (error, stdout) => {
      if (error) reject(new Error("Worker bundle encoding failed; use a matching platformd build"));
      else accept({ mainModule: worker.mainModule, bytes: stdout,
        sha256: createHash("sha256").update(stdout).digest("hex") });
    });
    // The exit callback owns failure reporting, including an early closed pipe.
    child.stdin?.on("error", () => {});
    child.stdin?.end(input);
  });
}
