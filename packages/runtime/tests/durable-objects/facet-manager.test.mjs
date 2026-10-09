import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const { FacetManager } = await importRuntime(
  "durable-objects/facet-manager.ts",
  {
    "cloudflare:workers": moduleUrl(
      "export class WorkerEntrypoint {constructor(ctx,env){this.ctx=ctx;this.env=env;}}",
    ),
    "../sockets/tunnel.js": moduleUrl(
      "export const inboundSocketAddress=()=>{},tunnelSockets=()=>{};",
    ),
  },
);

test("facet manager obtains a fresh host stub only during calls and preserves failures", async () => {
  let lookups = 0;
  const failure = new Error("DO_VERSION_STALE");
  const manager = new FacetManager(
    { props: { hostId: "host" } },
    {
      DO_HOST: {
        idFromString(id) {
          assert.equal(id, "host");
          return id;
        },
        get(id) {
          assert.equal(id, "host");
          lookups += 1;
          return {
            __openComputeCancelFacetConnect(token) {
              assert.equal(token, "token");
              if (lookups === 2) throw failure;
            },
          };
        },
      },
    },
  );
  assert.equal(lookups, 0);
  await manager.__openComputeCancelFacetConnect("token");
  assert.equal(lookups, 1);
  await assert.rejects(
    manager.__openComputeCancelFacetConnect("token"),
    (error) => error === failure,
  );
  assert.equal(lookups, 2);
});
