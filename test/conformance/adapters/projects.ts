import { relative } from "node:path";
import { COMPATIBILITY_DATE, COMPATIBILITY_FLAGS } from "./runtime-contract.ts";
import type { JsonRecord, PortableBinding, PortableFixture } from "./types.ts";

function isClassBinding(
  binding: PortableBinding,
): binding is Extract<
  PortableBinding,
  { readonly type: "do_namespace" | "workflow" }
> {
  return binding.type === "do_namespace" || binding.type === "workflow";
}

function exactBindingIds(
  fixture: PortableFixture,
  ids: Readonly<Record<string, string>>,
): Readonly<Record<string, string>> {
  const expected = Object.keys(fixture.bindings)
    .filter(
      (binding) =>
        fixture.bindings[binding]?.type === "kv_namespace" ||
        fixture.bindings[binding]?.type === "d1_database",
    )
    .sort();
  const actual = Object.keys(ids).sort();
  if (
    JSON.stringify(actual) !== JSON.stringify(expected) ||
    Object.values(ids).some((id) => id.length === 0)
  )
    throw new Error("portable fixture binding identities are incomplete");
  return ids;
}

export function openComputeProject(
  fixture: PortableFixture,
  name: string,
  accountId: string,
  bindingIds: Readonly<Record<string, string>> = {},
  bindingNames: Readonly<Record<string, string>> = {},
): JsonRecord {
  return workerProject(
    fixture,
    name,
    accountId,
    bindingIds,
    bindingNames,
    false,
  );
}

/** Minimal open-compute Wrangler project used before owned bindings are provisioned. */
export function openComputeBaseProject(
  fixture: PortableFixture,
  name: string,
  accountId: string,
): JsonRecord {
  return baseProject(fixture, name, accountId, false);
}

/** Minimal Worker project used before any owned binding has been provisioned. */
export function cloudflareBaseProject(
  fixture: PortableFixture,
  name: string,
  accountId: string,
): JsonRecord {
  return baseProject(fixture, name, accountId, true);
}

function baseProject(
  fixture: PortableFixture,
  name: string,
  accountId: string,
  workersDev: boolean,
): JsonRecord {
  return {
    name,
    main: relative(fixture.root, fixture.source),
    account_id: accountId,
    compatibility_date: COMPATIBILITY_DATE,
    compatibility_flags: [...COMPATIBILITY_FLAGS],
    workers_dev: workersDev,
    send_metrics: false,
  };
}

export function cloudflareProject(
  fixture: PortableFixture,
  name: string,
  accountId: string,
  bindingIds: Readonly<Record<string, string>> = {},
  bindingNames: Readonly<Record<string, string>> = {},
): JsonRecord {
  return workerProject(
    fixture,
    name,
    accountId,
    bindingIds,
    bindingNames,
    true,
  );
}

function workerProject(
  fixture: PortableFixture,
  name: string,
  accountId: string,
  bindingIds: Readonly<Record<string, string>>,
  bindingNames: Readonly<Record<string, string>>,
  workersDev: boolean,
): JsonRecord {
  const ids = exactBindingIds(fixture, bindingIds);
  const expectedNames = Object.keys(fixture.bindings)
    .filter(
      (binding) =>
        fixture.bindings[binding]?.type === "d1_database" ||
        fixture.bindings[binding]?.type === "r2_bucket" ||
        fixture.bindings[binding]?.type === "queue_producer" ||
        fixture.bindings[binding]?.type === "workflow",
    )
    .sort();
  const actualNames = Object.keys(bindingNames).sort();
  if (
    JSON.stringify(expectedNames) !== JSON.stringify(actualNames) ||
    Object.values(bindingNames).some((bindingName) => bindingName.length === 0)
  ) {
    throw new Error(
      "portable fixture named Cloudflare bindings are incomplete",
    );
  }
  const durableBindings = Object.entries(fixture.bindings)
    .filter(
      (
        entry,
      ): entry is [
        string,
        Extract<
          PortableBinding,
          { readonly type: "do_namespace" | "workflow" }
        >,
      ] => isClassBinding(entry[1]) && entry[1].type === "do_namespace",
    )
    .map(([binding, value]) => ({
      name: binding,
      class_name: value.className,
    }));
  const queueBindings = Object.entries(fixture.bindings)
    .filter(([, value]) => value.type === "queue_producer")
    .map(([binding]) => ({ binding, queue: bindingNames[binding] }));
  const workflowBindings = Object.entries(fixture.bindings)
    .filter(
      (
        entry,
      ): entry is [
        string,
        Extract<
          PortableBinding,
          { readonly type: "do_namespace" | "workflow" }
        >,
      ] => isClassBinding(entry[1]) && entry[1].type === "workflow",
    )
    .map(([binding, value]) => ({
      binding,
      name: bindingNames[binding],
      class_name: value.className,
      ...(value.schedules === undefined ? {} : { schedules: value.schedules }),
    }));
  return {
    ...baseProject(fixture, name, accountId, workersDev),
    ...(Object.values(fixture.bindings).some(
      (binding) => binding.type === "worker_loader",
    )
      ? {
          worker_loaders: Object.entries(fixture.bindings)
            .filter(([, value]) => value.type === "worker_loader")
            .map(([binding]) => ({ binding })),
        }
      : {}),
    kv_namespaces: Object.entries(fixture.bindings)
      .filter(([, value]) => value.type === "kv_namespace")
      .map(([binding]) => ({ binding, id: ids[binding] })),
    d1_databases: Object.entries(fixture.bindings)
      .filter(([, value]) => value.type === "d1_database")
      .map(([binding]) => ({
        binding,
        database_id: ids[binding],
        database_name: bindingNames[binding],
      })),
    r2_buckets: Object.entries(fixture.bindings)
      .filter(([, value]) => value.type === "r2_bucket")
      .map(([binding]) => ({ binding, bucket_name: bindingNames[binding] })),
    ...(durableBindings.length === 0
      ? {}
      : {
          durable_objects: { bindings: durableBindings },
          migrations: [
            {
              tag: "v1",
              new_sqlite_classes: [
                ...new Set(durableBindings.map((value) => value.class_name)),
              ],
            },
          ],
        }),
    ...(queueBindings.length === 0
      ? {}
      : { queues: { producers: queueBindings } }),
    ...(workflowBindings.length === 0 ? {} : { workflows: workflowBindings }),
  };
}

/** Return JSON with recursively sorted object keys for stable cross-provider comparison. */
