import type {
  AccountId,
  CreateDeploymentUploadBody,
  FinalizeDeploymentUploadBody,
  OperatorClient,
  WorkerId,
} from "@open-compute/operator-sdk";
import { parseDeploymentUploadId, parseSha256Digest } from "@open-compute/operator-sdk";
import { sha256Bytes } from "./hash";

const MIME: Record<string, string> = {
  css: "text/css; charset=utf-8",
  html: "text/html; charset=utf-8",
  htm: "text/html; charset=utf-8",
  js: "text/javascript; charset=utf-8",
  mjs: "text/javascript; charset=utf-8",
  json: "application/json; charset=utf-8",
  png: "image/png",
  svg: "image/svg+xml",
  txt: "text/plain; charset=utf-8",
  wasm: "application/wasm",
  webp: "image/webp",
};

const DEFAULT_ASSET_ROUTING = {
  schemaVersion: 1 as const,
  runWorkerFirst: false,
  htmlHandling: "auto-trailing-slash" as const,
  notFoundHandling: "404-page" as const,
  headers: [],
  redirects: [],
};

function assetPath(file: File): string {
  const relative = file.webkitRelativePath.trim();
  const raw = relative.includes("/") ? relative.split("/").slice(1).join("/") : relative || file.name;
  const normalized = raw.replaceAll("\\", "/").replace(/^\/+/, "");
  return `/${normalized.split("/").map(segment => encodeURIComponent(segment)).join("/")}`;
}

function contentType(path: string): string {
  const extension = path.split(".").at(-1)?.toLowerCase() ?? "";
  return MIME[extension] ?? "application/octet-stream";
}

async function buildAssetManifest(files: File[]) {
  const entries = await Promise.all(files.map(async file => {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const path = assetPath(file);
    return {
      path,
      sha256: await sha256Bytes(bytes),
      size: bytes.byteLength,
      contentType: contentType(path),
      bytes,
    };
  }));
  entries.sort((left, right) => left.path.localeCompare(right.path));
  return {
    manifest: {
      schemaVersion: 1 as const,
      entries: entries.map(entry => ({
        path: entry.path,
        sha256: entry.sha256,
        size: entry.size,
        contentType: entry.contentType,
      })),
    },
    blobs: new Map(entries.map(entry => [entry.sha256, entry.bytes])),
  };
}

export async function uploadWorkerWithAssets(input: {
  client: OperatorClient;
  accountId: AccountId;
  workerId: WorkerId;
  bundleBytes: Uint8Array;
  assetFiles: File[];
  mainModule: string;
  promote: boolean;
}) {
  const bundleSha256 = await sha256Bytes(input.bundleBytes);
  const { manifest, blobs } = await buildAssetManifest(input.assetFiles);
  const createBody: CreateDeploymentUploadBody = {
    contentKind: "worker",
    bundle: {
      sha256: parseSha256Digest(bundleSha256),
      size: input.bundleBytes.byteLength,
    },
    manifest,
    routing: DEFAULT_ASSET_ROUTING,
  };
  const idempotencyKey = crypto.randomUUID();
  const session = await input.client.workers.createDeploymentUpload({
    accountId: input.accountId,
    workerId: input.workerId,
    body: createBody,
    idempotencyKey,
  });
  const uploadId = parseDeploymentUploadId(session.id);
  try {
    for (const object of session.objects) {
      if (object.verified) continue;
      let body: Uint8Array;
      if (object.kind === "bundle") {
        if (object.sha256 !== bundleSha256 || object.size !== input.bundleBytes.byteLength) {
          throw new Error("Deployment upload bundle inventory changed.");
        }
        body = input.bundleBytes;
      } else if (object.kind === "asset_blob") {
        const asset = blobs.get(object.sha256);
        if (!asset || asset.byteLength !== object.size) {
          throw new Error("Deployment upload asset inventory changed.");
        }
        body = asset;
      } else if (object.kind === "asset_manifest") {
        continue;
      } else {
        throw new Error(`Unsupported deployment upload object kind: ${object.kind}`);
      }
      await input.client.workers.putDeploymentUploadObject({
        accountId: input.accountId,
        workerId: input.workerId,
        uploadId,
        sha256: parseSha256Digest(object.sha256),
        body,
      });
    }
    const finalizeBody: FinalizeDeploymentUploadBody = {
      mainModule: input.mainModule,
      vars: {},
      secrets: {},
      bindings: {},
      services: {},
      promote: input.promote,
    };
    return await input.client.workers.finalizeDeploymentUpload({
      accountId: input.accountId,
      workerId: input.workerId,
      uploadId,
      body: finalizeBody,
      idempotencyKey: crypto.randomUUID(),
    });
  } catch (error) {
    try {
      await input.client.workers.abortDeploymentUpload({
        accountId: input.accountId,
        workerId: input.workerId,
        uploadId,
      });
    } catch {
      // Preserve the upload/finalize failure that caused cleanup.
    }
    throw error;
  }
}
