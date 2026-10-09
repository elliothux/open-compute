import assert from "node:assert/strict";
import test from "node:test";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

// Unrelated entrypoints are not invoked by cancellation; only module loading is stubbed.
const unused = moduleUrl(`
export const DoHost=class {}, FacetManager=class {}, AiSearchTransport=class {}, AiTransport=class {}, CacheTransport=class {},
CacheWriteTransport=class {}, ImageTransport=class {}, KVNamespace=class {}, ExtensionCacheTransport=class {},
PrivateHttpTransport=class {}, ServiceFetchCompletion=class {}, ServiceTransport=class {}, VectorizeTransport=class {},
WorkflowBindingTransport=class {}, AlarmIndex=class {}, ArtifactsTransport=class {}, AssetTransport=class {},
D1Transport=class {}, DoTransport=class {}, QueueTransport=class {}, R2Transport=class {}, BrowserTransport=class {};
`);
const imports = {
  "cloudflare:workers": moduleUrl(
    "export class WorkerEntrypoint {constructor(_ctx, env) {this.env=env;}}",
  ),
  "../loader/shared.js": moduleUrl(
    "export const currentStartupGeneration=()=> 'generation'; export const stableCode=error=>error.stableCode;",
  ),
  "../observability/collector.js": moduleUrl(
    "export const collectObservabilityTail=()=>{};",
  ),
  "../sockets/tunnel.js": moduleUrl(
    "export const inboundSocketAddress=()=>{}, tunnelSockets=()=>{}, validateSocketAuthorityWire=()=>{};",
  ),
  "./admission.js": moduleUrl(
    "export const admitted=()=>{throw Object.assign(new Error('admission full'),{stableCode:'DO_STORAGE_LIMIT'});};",
  ),
  "./identity.js": moduleUrl(
    await compileRuntime("durable-objects/identity.ts"),
  ),
};
for (const path of [
  "./host.js",
  "./facet-manager.js",
  "../ai-search/host.js",
  "../ai/host.js",
  "../browser/host.js",
  "../cache/host.js",
  "../images/host.js",
  "../kv/transport.js",
  "../services/transport.js",
  "../vectorize/host.js",
  "../workflows/binding.js",
  "../loader/transports.js",
])
  imports[path] = unused;
const { default: DoRouter } = await importRuntime(
  "durable-objects/router.ts",
  imports,
);

test("authenticated order cancellation remains available when tenant admission is full", async () => {
  const authority = {
    instanceId: "019c0000000070008000000000000001",
    workerId: "019c0000-0000-7000-8000-000000000002",
    versionId: "019c0000-0000-7000-8000-000000000003",
    workerCodeSha256: "a".repeat(64),
    namespaceResourceId: "019c0000-0000-7000-8000-000000000004",
    objectId: "b".repeat(64),
    className: "Counter",
    hostKey: "isolated-host",
    objectGeneration: 1,
    routeGeneration: 1,
  };
  const identity = {
    "x-open-compute-startup-generation": "generation",
    "x-open-compute-version-id": authority.versionId,
    "x-open-compute-descriptor-sha256": "c".repeat(64),
    "x-open-compute-request-id": authority.workerId,
    "x-open-compute-do-operation": "rpc",
    "x-open-compute-binding-id": authority.namespaceResourceId,
    "x-open-compute-object-id": authority.objectId,
    "x-open-compute-do-order-channel": "d".repeat(32),
    "x-open-compute-do-order-sequence": "1",
  };
  let cancelled,
    authorized = true;
  const env = {
    BINDING_BACKEND_TOKEN: "test-only-backend-token",
    BINDING_BACKEND: {
      fetch: async () =>
        authorized
          ? Response.json(authority)
          : new Response(null, {
              status: 403,
              headers: {
                "x-open-compute-binding-error": "DO_INTERNAL_PROTOCOL_ERROR",
              },
            }),
    },
    DO_HOST: {
      idFromName: (key) => key,
      get: (key) => {
        assert.equal(key, authority.hostKey);
        return {
          __openComputeCancelOrder: async (order) => {
            cancelled = order;
          },
        };
      },
    },
  };
  const router = new DoRouter({}, env);
  await router.cancelOrder(identity);
  assert.deepEqual(cancelled, { channelId: "d".repeat(32), sequence: 1 });
  cancelled = undefined;
  authorized = false;
  await assert.rejects(router.cancelOrder(identity));
  assert.equal(cancelled, undefined);
});
