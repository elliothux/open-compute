import assert from "node:assert/strict";
import test from "node:test";
import {
  createOperatorClient,
  OperatorApiError,
  OperatorProtocolError,
  parseAccountId,
  parseQueueId,
  parseResourceId,
  parseWorkerId,
} from "../dist/index.js";

const baseUrl = process.env.OPEN_COMPUTE_OPERATOR_BASE_URL;
const token = process.env.OPEN_COMPUTE_ADMIN_TOKEN;
const contractScope = process.env.OPEN_COMPUTE_OPERATOR_CONTRACT_SCOPE ?? "full";

if (!baseUrl || !token) {
  test("contract-ocd requires live admin router env", { skip: true }, () => {});
} else {
  const client = createOperatorClient({
    baseUrl: new URL(baseUrl),
    getAccessToken: () => token,
  });

  test("system.meta matches strict SDK schema against live admin router", async () => {
    const meta = await client.system.meta();
    assert.equal(meta.apiVersion, "v1");
    assert.equal(typeof meta.release, "string");
    assert.ok(Array.isArray(meta.capabilities));
  });

  test("system.account matches strict SDK schema against live admin router", async () => {
    const account = await client.system.account();
    assert.match(account.accountId, /^[0-9a-f-]{36}$/);
  });

  test("system.status matches strict SDK schema against live admin router", async () => {
    const status = await client.system.status();
    assert.equal(typeof status.readiness, "string");
    assert.ok(Array.isArray(status.components));
  });

  test("unauthorized requests fail before schema validation", async () => {
    const unauthenticated = createOperatorClient({
      baseUrl: new URL(baseUrl),
      getAccessToken: () => null,
    });
    await assert.rejects(
      unauthenticated.system.meta(),
      error => {
        assert.ok(error instanceof OperatorApiError);
        assert.equal(error.code, "admin_auth_required");
        return true;
      },
    );
  });

  test("catalog list operations validate account scope against live admin router", async () => {
    const account = await client.system.account();
    const accountId = parseAccountId(account.accountId);
    await client.kv.listNamespaces({ accountId });
    await client.kv.listBackups({ accountId });
    await client.d1.listDatabases({ accountId });
    await client.r2.listBuckets({ accountId });
    await client.durableObjects.listNamespaces({ accountId });
    await client.queues.list({ accountId });
    await client.workflows.list({ accountId });
  });

  test("catalog filters, sort, and opaque cursor pagination are server-authoritative", async () => {
    const account = await client.system.account();
    const accountId = parseAccountId(account.accountId);
    const suffix = crypto.randomUUID().slice(0, 8);
    const workers = [];
    const namespaces = [];
    try {
      for (const label of ["zulu", "alpha", "middle"]) {
        const created = await client.workers.create({
          accountId,
          name: `catalog-${label}-${suffix}`,
          idempotencyKey: crypto.randomUUID(),
        });
        workers.push(parseWorkerId(created.worker.id));
        const namespace = await client.kv.createNamespace({
          accountId,
          name: `catalog-${label}-${suffix}`,
          idempotencyKey: crypto.randomUUID(),
        });
        namespaces.push(parseResourceId(namespace.resourceId));
      }

      const firstWorkers = await client.workers.list({
        accountId,
        search: suffix,
        deployed: false,
        sort: "name",
        direction: "asc",
        limit: 1,
      });
      assert.equal(firstWorkers.workers.length, 1);
      assert.match(firstWorkers.workers[0].name, /catalog-alpha-/);
      assert.equal(firstWorkers.workers[0].routeCount, 1);
      assert.equal(firstWorkers.workers[0].traffic.requests, 0);
      assert.ok(firstWorkers.cursor);
      const secondWorkers = await client.workers.list({
        accountId,
        search: suffix,
        deployed: false,
        sort: "name",
        direction: "asc",
        cursor: firstWorkers.cursor,
        limit: 1,
      });
      assert.match(secondWorkers.workers[0].name, /catalog-middle-/);

      const firstNamespaces = await client.kv.listNamespaces({
        accountId,
        search: suffix,
        status: "ready",
        sort: "name",
        direction: "desc",
        limit: 1,
      });
      assert.equal(firstNamespaces.namespaces.length, 1);
      assert.match(firstNamespaces.namespaces[0].resource.name, /catalog-zulu-/);
      assert.ok(firstNamespaces.cursor);
      const secondNamespaces = await client.kv.listNamespaces({
        accountId,
        search: suffix,
        status: "ready",
        sort: "name",
        direction: "desc",
        cursor: firstNamespaces.cursor,
        limit: 1,
      });
      assert.match(secondNamespaces.namespaces[0].resource.name, /catalog-middle-/);
    } finally {
      for (const namespaceId of namespaces) {
        await client.kv.deleteNamespace({
          accountId,
          namespaceId,
          idempotencyKey: crypto.randomUUID(),
        });
      }
      for (const workerId of workers) {
        await client.workers.delete({
          accountId,
          workerId,
          idempotencyKey: crypto.randomUUID(),
        });
      }
    }
  });

  test("D1, R2, Queue, and Workflow catalogs page without gaps or duplicates", async () => {
    const accountId = parseAccountId((await client.system.account()).accountId);
    const suffix = crypto.randomUUID().slice(0, 8);
    const d1 = [];
    const r2 = [];
    const queues = [];
    const workflows = [];
    try {
      for (const label of ["alpha", "zulu"]) {
        d1.push(await client.d1.createDatabase({
          accountId,
          name: `page-d1-${label}-${suffix}`,
          idempotencyKey: crypto.randomUUID(),
        }));
        r2.push(await client.r2.createBucket({
          accountId,
          name: `page-r2-${label}-${suffix}`,
          idempotencyKey: crypto.randomUUID(),
        }));
        queues.push(await client.queues.create({
          accountId,
          name: `page-queue-${label}-${suffix}`,
          deliveryDelaySeconds: 0,
          retentionSeconds: 120,
          maxBacklogBytes: 1_048_576,
          idempotencyKey: crypto.randomUUID(),
        }));
        workflows.push(await client.workflows.create({
          accountId,
          name: `page-workflow-${label}-${suffix}`,
        }));
      }

      const cases = [
        {
          first: () => client.d1.listDatabases({ accountId, search: suffix, sort: "name", direction: "asc", limit: 1 }),
          second: cursor => client.d1.listDatabases({ accountId, search: suffix, sort: "name", direction: "asc", cursor, limit: 1 }),
          rows: result => result.databases.map(row => row.resource.name),
        },
        {
          first: () => client.r2.listBuckets({ accountId, search: suffix, sort: "name", direction: "asc", limit: 1 }),
          second: cursor => client.r2.listBuckets({ accountId, search: suffix, sort: "name", direction: "asc", cursor, limit: 1 }),
          rows: result => result.buckets.map(row => row.name),
        },
        {
          first: () => client.queues.list({ accountId, search: suffix, sort: "name", direction: "asc", limit: 1 }),
          second: cursor => client.queues.list({ accountId, search: suffix, sort: "name", direction: "asc", cursor, limit: 1 }),
          rows: result => result.queues.map(row => row.name),
        },
        {
          first: () => client.workflows.list({ accountId, search: suffix, sort: "name", direction: "asc", limit: 1 }),
          second: cursor => client.workflows.list({ accountId, search: suffix, sort: "name", direction: "asc", cursor, limit: 1 }),
          rows: result => result.workflows.map(row => row.name),
        },
      ];
      for (const catalog of cases) {
        const first = await catalog.first();
        assert.ok(first.nextCursor ?? first.cursor);
        const second = await catalog.second(first.nextCursor ?? first.cursor);
        const names = [...catalog.rows(first), ...catalog.rows(second)];
        assert.equal(names.length, 2);
        assert.equal(new Set(names).size, 2);
        assert.match(names[0], /alpha/);
        assert.match(names[1], /zulu/);
      }
    } finally {
      for (const workflow of workflows) {
        await client.workflows.delete({ accountId, workflowId: workflow.id });
      }
      for (const queue of queues) {
        await client.queues.delete({
          accountId,
          queueId: parseQueueId(queue.queue.id),
          expectedLifecycleGeneration: queue.queue.lifecycleGeneration,
          idempotencyKey: crypto.randomUUID(),
        });
      }
      for (const bucket of r2) {
        await client.r2.deleteBucket({
          accountId,
          bucketId: parseResourceId(bucket.bucket.resourceId),
          idempotencyKey: crypto.randomUUID(),
        });
      }
      for (const database of d1) {
        await client.d1.deleteDatabase({
          accountId,
          databaseId: parseResourceId(database.resourceId),
          idempotencyKey: crypto.randomUUID(),
        });
      }
    }
  });

  test("catalog lifecycle round-trip validates against live admin router", async () => {
    const account = await client.system.account();
    const accountId = parseAccountId(account.accountId);
    const suffix = crypto.randomUUID().slice(0, 8);

    const kv = await client.kv.createNamespace({
      accountId,
      name: `contract-kv-${suffix}`,
      idempotencyKey: crypto.randomUUID(),
    });
    assert.ok(kv.resourceId);
    const renamedKv = await client.kv.renameNamespace({
      accountId,
      namespaceId: parseResourceId(kv.resourceId),
      name: `contract-kv-renamed-${suffix}`,
    });
    assert.equal(renamedKv.namespace.name, `contract-kv-renamed-${suffix}`);
    const fetchedKv = await client.kv.getNamespace({
      accountId,
      namespaceId: parseResourceId(kv.resourceId),
    });
    assert.equal(fetchedKv.namespace.resource.name, `contract-kv-renamed-${suffix}`);
    const key = `contract/${suffix}`;
    await client.kv.putValue({
      accountId,
      namespaceId: parseResourceId(kv.resourceId),
      key,
      value: "live contract value",
      metadata: { source: "contract" },
      expirationTtl: 120,
      idempotencyKey: crypto.randomUUID(),
    });
    const value = await client.kv.getValue({
      accountId,
      namespaceId: parseResourceId(kv.resourceId),
      key,
    });
    assert.equal(value.value, "live contract value");
    assert.deepEqual(value.metadata, { source: "contract" });
    await client.kv.deleteValue({
      accountId,
      namespaceId: parseResourceId(kv.resourceId),
      key,
      idempotencyKey: crypto.randomUUID(),
    });
    await client.kv.deleteNamespace({
      accountId,
      namespaceId: parseResourceId(kv.resourceId),
      idempotencyKey: crypto.randomUUID(),
    });

    const d1 = await client.d1.createDatabase({
      accountId,
      name: `contract-d1-${suffix}`,
      idempotencyKey: crypto.randomUUID(),
    });
    assert.ok(d1.resourceId);
    await client.d1.deleteDatabase({
      accountId,
      databaseId: parseResourceId(d1.resourceId),
      idempotencyKey: crypto.randomUUID(),
    });

    const workflow = await client.workflows.create({
      accountId,
      name: `contract-wf-${suffix}`,
    });
    assert.ok(workflow.id);
    await client.workflows.delete({ accountId, workflowId: workflow.id });
  });

  test("queue configuration lifecycle validates against live admin router", async () => {
    const account = await client.system.account();
    const accountId = parseAccountId(account.accountId);
    const created = await client.queues.create({
      accountId,
      name: `contract-queue-${crypto.randomUUID().slice(0, 8)}`,
      deliveryDelaySeconds: 2,
      retentionSeconds: 120,
      maxBacklogBytes: 1_048_576,
      idempotencyKey: crypto.randomUUID(),
    });
    const queueId = parseQueueId(created.queue.id);
    const updated = await client.queues.updateConfig({
      accountId,
      queueId,
      expectedConfigGeneration: created.queue.configGeneration,
      retentionSeconds: 180,
      idempotencyKey: crypto.randomUUID(),
    });
    assert.equal(updated.queue.retentionSeconds, 180);
    await client.queues.delete({
      accountId,
      queueId,
      expectedLifecycleGeneration: updated.queue.lifecycleGeneration,
      idempotencyKey: crypto.randomUUID(),
    });
  });

  test("platform cache and workflow maintenance operations match live schemas", async () => {
    const account = await client.system.account();
    const accountId = parseAccountId(account.accountId);
    const created = await client.workers.create({
      accountId,
      name: `contract-cache-${crypto.randomUUID().slice(0, 8)}`,
      idempotencyKey: crypto.randomUUID(),
    });
    const workerId = parseWorkerId(created.worker.id);
    try {
      if (contractScope === "router") {
        for (const [label, operation] of [
          ["worker cache", () => client.platform.workerCache({ accountId, workerId })],
          ["cache GC", () => client.platform.cacheGc()],
          ["scheduler repair", () => client.platform.repairScheduler()],
        ]) {
          await assert.rejects(operation(), error => {
            assert.ok(
              error instanceof OperatorApiError,
              `${label} returned ${error?.constructor?.name ?? typeof error}: ${error?.message ?? "unknown error"}`,
            );
            assert.equal(error.code, "platform_unavailable", label);
            return true;
          });
        }
        await assert.rejects(client.workflows.reconcile(), error => {
          assert.ok(error instanceof OperatorApiError);
          assert.equal(error.code, "workflow_runtime_unavailable");
          return true;
        });
        return;
      }
      const cache = await client.platform.workerCache({ accountId, workerId });
      assert.equal(typeof cache.entries, "number");
      const purged = await client.platform.purgeWorkerCache({ accountId, workerId });
      assert.equal(purged.success, true);
      const gc = await client.platform.cacheGc();
      assert.equal(typeof gc.deleted, "number");
      const repair = await client.platform.repairScheduler();
      assert.equal(typeof repair.alarmRepaired, "number");
      assert.equal(repair.repaired, repair.alarmRepaired + repair.productRepaired);
      assert.equal(await client.workflows.reconcile(), null);
    } finally {
      await client.workers.delete({
        accountId,
        workerId,
        idempotencyKey: crypto.randomUUID(),
      });
    }
  });

  test("invalid catalog params fail before network I/O", () => {
    assert.throws(
      () => client.kv.listNamespaces({ accountId: "" }),
      error => error instanceof OperatorProtocolError,
    );
  });

  test("malformed list queries return canonical operator errors", async () => {
    const accountId = parseAccountId((await client.system.account()).accountId);
    const resourceId = "01900000-0000-7000-8000-000000000001";
    const paths = [
      `accounts/${accountId}/workers?sort=invalid`,
      `accounts/${accountId}/kv/namespaces?sort=invalid`,
      `accounts/${accountId}/d1/databases?sort=invalid`,
      `accounts/${accountId}/r2/buckets?sort=invalid`,
      `accounts/${accountId}/durable-objects/namespaces?sort=invalid`,
      `accounts/${accountId}/kv/namespaces/${resourceId}/keys?limit=invalid`,
      `accounts/${accountId}/r2/buckets/${resourceId}/objects?limit=invalid`,
      `accounts/${accountId}/durable-objects/namespaces/${resourceId}/objects?limit=invalid`,
    ];
    for (const path of paths) {
      const response = await fetch(new URL(path, baseUrl), {
        headers: { authorization: `Bearer ${token}` },
      });
      assert.equal(response.status, 400, path);
      assert.match(response.headers.get("content-type") ?? "", /application\/json/, path);
      const payload = await response.json();
      assert.equal(payload.ok, false, path);
      assert.equal(payload.error.code, "CONFIG_INVALID", path);
      assert.match(payload.error.requestId, /^[0-9a-f-]{36}$/, path);
    }
  });
}
