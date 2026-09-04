import { createClient } from "cloudflare/tree-shakable";
import { BaseAccounts } from "cloudflare/resources/accounts/accounts";
import { BaseScripts } from "cloudflare/resources/workers/scripts/scripts";
import { BaseDeployments } from "cloudflare/resources/workers/scripts/deployments";
import { BaseVersions as BaseWorkerVersions } from "cloudflare/resources/workers/scripts/versions";
import { BaseNamespaces } from "cloudflare/resources/kv/namespaces/namespaces";
import { BaseKeys } from "cloudflare/resources/kv/namespaces/keys";
import { BaseValues } from "cloudflare/resources/kv/namespaces/values";
import { BaseDatabase } from "cloudflare/resources/d1/database/database";
import { BaseBuckets } from "cloudflare/resources/r2/buckets/buckets";
import { BaseQueues } from "cloudflare/resources/queues/queues";
import { BaseConsumers } from "cloudflare/resources/queues/consumers";
import { BaseWorkflows } from "cloudflare/resources/workflows/workflows";
import { BaseInstances } from "cloudflare/resources/workflows/instances/instances";
import { BaseEvents } from "cloudflare/resources/workflows/instances/events";
import { BaseStatus } from "cloudflare/resources/workflows/instances/status";
import { BaseVersions as BaseWorkflowVersions } from "cloudflare/resources/workflows/versions";
import { BaseTelemetry } from "cloudflare/resources/workers/observability/telemetry";
import { createOpenComputeExtension } from "@open-compute/cloudflare-extension";

const resources = [
  BaseAccounts,
  BaseScripts,
  BaseDeployments,
  BaseWorkerVersions,
  BaseNamespaces,
  BaseKeys,
  BaseValues,
  BaseDatabase,
  BaseBuckets,
  BaseQueues,
  BaseConsumers,
  BaseWorkflows,
  BaseInstances,
  BaseEvents,
  BaseStatus,
  BaseWorkflowVersions,
  BaseTelemetry,
] as const;

/** Create the browser management client with the official Cloudflare transport. */
export function createManagementClient(token: string) {
  const cloudflare = createClient({
    apiToken: token,
    baseURL: new URL("/client/v4", window.location.origin).href,
    maxRetries: 0,
    resources,
  });
  return { cloudflare, openCompute: createOpenComputeExtension(cloudflare) } as const;
}

export type ManagementClient = ReturnType<typeof createManagementClient>;
