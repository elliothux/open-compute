import type { QueryClient } from "@tanstack/react-query";
import type { OpenComputeJsonValue } from "@open-compute/sdk";
import type { ManagementClient } from "../lib/cloudflare";

type ResourceBindingUpsert =
  | {
      type: "kv_namespace";
      originalName: string | null;
      name: string;
      namespaceId: string;
    }
  | {
      type: "r2_bucket";
      originalName: string | null;
      name: string;
      bucketName: string;
    }
  | {
      type: "d1";
      originalName: string | null;
      name: string;
      databaseId: string;
    }
  | {
      type: "durable_object_namespace";
      originalName: string | null;
      name: string;
      className: string;
      namespaceId: string;
    }
  | {
      type: "queue";
      originalName: string | null;
      name: string;
      queueName: string;
    }
  | {
      type: "service";
      originalName: string | null;
      name: string;
      service: string;
      entrypoint?: string;
      props?: { readonly [key: string]: OpenComputeJsonValue };
    }
  | {
      type: "vectorize";
      originalName: string | null;
      name: string;
      indexName: string;
    }
  | {
      type: "images" | "ai" | "version_metadata" | "worker_loader";
      originalName: string | null;
      name: string;
    }
  | {
      type: "ai_search" | "ai_search_namespace";
      originalName: string | null;
      name: string;
      resourceId: string;
    };

export type BindingKind = ResourceBindingUpsert["type"];
export type ResourceBindingChange =
  | ResourceBindingUpsert
  | { type: BindingKind; originalName: string; remove: true };

export const resourceBindingLabels = {
  kv_namespace: "KV namespace",
  r2_bucket: "R2 bucket",
  d1: "D1 database",
  durable_object_namespace: "Durable Object",
  queue: "Queue",
  service: "Service binding",
  vectorize: "Vectorize index",
  images: "Images",
  ai: "Workers AI",
  version_metadata: "Version metadata",
  worker_loader: "Dynamic Workers",
  ai_search: "AI Search",
  ai_search_namespace: "AI Search namespace",
} satisfies Record<BindingKind, string>;

export function invalidateWorkerBindingQueries(
  queryClient: QueryClient,
  selectedInstanceId: string,
  workerId: string,
) {
  return Promise.all(
    ["version-settings", "versions", "deployments"].map((key) =>
      queryClient.invalidateQueries({
        queryKey: [
          "cloudflare-v4",
          "workers",
          selectedInstanceId,
          workerId,
          key,
        ],
      }),
    ),
  );
}

/** Save one resource binding as an immutable Version, then deploy only that verified Version. */
export async function saveWorkerResourceBinding(
  client: ManagementClient,
  selectedInstanceId: string,
  workerId: string,
  bindingNames: readonly string[],
  change: ResourceBindingChange,
) {
  const listIds = async () => {
    const ids = new Set<string>();
    for await (const version of client.workers.scripts.versions.list(workerId, {
      account_id: selectedInstanceId,
      deployable: true,
    })) {
      if (!version.id) throw new Error("Worker Version has no ID.");
      ids.add(version.id);
    }
    return ids;
  };
  const before = await listIds();
  const inherited = bindingNames
    .filter((name) => name !== change.originalName)
    .map((name) => ({ type: "inherit" as const, name }));
  const changed =
    "remove" in change
      ? []
      : change.type === "kv_namespace"
        ? [
            {
              type: change.type,
              name: change.name.trim(),
              namespace_id: change.namespaceId,
            },
          ]
        : change.type === "r2_bucket"
          ? [
              {
                type: change.type,
                name: change.name.trim(),
                bucket_name: change.bucketName,
              },
            ]
          : change.type === "d1"
            ? [
                {
                  type: change.type,
                  name: change.name.trim(),
                  database_id: change.databaseId,
                },
              ]
            : change.type === "durable_object_namespace"
              ? [
                  {
                    type: change.type,
                    name: change.name.trim(),
                    class_name: change.className,
                  },
                ]
              : change.type === "queue"
                ? [
                    {
                      type: change.type,
                      name: change.name.trim(),
                      queue_name: change.queueName,
                    },
                  ]
                : change.type === "service"
                  ? [
                      {
                        type: change.type,
                        name: change.name.trim(),
                        service: change.service,
                        ...(change.entrypoint
                          ? { entrypoint: change.entrypoint }
                          : {}),
                        ...(change.props ? { props: change.props } : {}),
                      },
                    ]
                  : change.type === "vectorize"
                    ? [
                        {
                          type: change.type,
                          name: change.name.trim(),
                          index_name: change.indexName,
                        },
                      ]
                    : change.type === "ai_search"
                      ? [
                          {
                            type: change.type,
                            name: change.name.trim(),
                            instance_name: change.resourceId,
                          },
                        ]
                      : change.type === "ai_search_namespace"
                        ? [
                            {
                              type: change.type,
                              name: change.name.trim(),
                              namespace: change.resourceId,
                            },
                          ]
                        : [{ type: change.type, name: change.name.trim() }];
  const label =
    "remove" in change ? "Delete" : change.originalName ? "Edit" : "Add";
  const kind = resourceBindingLabels[change.type];
  const message = `${label} ${kind}${change.type === "service" ? "" : " binding"}`;
  await client.workers.scripts.scriptAndVersionSettings.edit(workerId, {
    account_id: selectedInstanceId,
    settings: {
      bindings: [...inherited, ...changed],
      annotations: {
        "workers/message": `${message} ${change.originalName || ("remove" in change ? "" : change.name.trim())}`,
      },
    },
  });
  const created = [...(await listIds())].filter((id) => !before.has(id));
  if (created.length !== 1)
    throw new Error(
      "The new Worker Version could not be identified uniquely. It was saved but not deployed.",
    );
  const version = await client.workers.scripts.versions.get(created[0]!, {
    account_id: selectedInstanceId,
    script_name: workerId,
  });
  const matching = (version.resources.bindings ?? []).find(
    (binding) =>
      binding.type === change.type &&
      binding.name ===
        ("remove" in change ? change.originalName : change.name.trim()),
  );
  const verified =
    "remove" in change
      ? matching === undefined
      : change.type === "kv_namespace"
        ? matching?.type === "kv_namespace" &&
          matching.namespace_id === change.namespaceId
        : change.type === "r2_bucket"
          ? matching?.type === "r2_bucket" &&
            matching.bucket_name === change.bucketName
          : change.type === "d1"
            ? matching?.type === "d1" &&
              matching.database_id === change.databaseId
            : change.type === "durable_object_namespace"
              ? matching?.type === "durable_object_namespace" &&
                matching.class_name === change.className &&
                matching.namespace_id === change.namespaceId
              : change.type === "queue"
                ? matching?.type === "queue" &&
                  matching.queue_name === change.queueName
                : change.type === "service"
                  ? matching?.type === "service" &&
                    matching.service === change.service &&
                    (matching.entrypoint ?? undefined) === change.entrypoint &&
                    canonicalJson(
                      matching && "props" in matching
                        ? matching.props
                        : undefined,
                    ) === canonicalJson(change.props)
                  : change.type === "vectorize"
                    ? matching?.type === "vectorize" &&
                      matching.index_name === change.indexName
                    : change.type === "ai_search"
                      ? matching?.type === "ai_search" &&
                        matching.instance_name === change.resourceId
                      : change.type === "ai_search_namespace"
                        ? matching?.type === "ai_search_namespace" &&
                          matching.namespace === change.resourceId
                        : matching?.type === change.type;
  if (!verified)
    throw new Error(
      `The saved Worker Version does not match the requested ${kind}${change.type === "service" ? "" : " binding"}. It was not deployed.`,
    );
  await client.workers.scripts.deployments.create(workerId, {
    account_id: selectedInstanceId,
    strategy: "percentage",
    versions: [{ version_id: created[0]!, percentage: 100 }],
    annotations: { "workers/message": message },
  });
}

function canonicalJson(value: unknown): string | undefined {
  if (value === undefined) return undefined;
  return JSON.stringify(value, (_key, item: unknown) =>
    item && typeof item === "object" && !Array.isArray(item)
      ? Object.fromEntries(
          Object.entries(item).sort(([a], [b]) => a.localeCompare(b)),
        )
      : item,
  );
}
