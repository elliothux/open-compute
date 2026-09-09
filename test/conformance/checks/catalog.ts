import type { JsonRecord } from "../adapters/types.ts";
import {
  array,
  baseline,
  capabilities,
  catalog,
  contracts,
  digest,
  json,
  productNames,
  record,
  sourceIdentity,
  string,
  strings,
} from "./context.ts";

export function baselineIdentity(): void {
  const value = baseline();
  if (value.schemaVersion !== 1) throw new Error("unsupported baseline schema");
  const hash = string(value.openComputeRevision, "openComputeRevision");
  const actualSource = sourceIdentity();
  if (!/^[0-9a-f]{64}$/.test(hash) || hash !== actualSource) {
    throw new Error(
      `open-compute source digest drift: expected=${hash}; actual=${actualSource}`,
    );
  }
  if (
    string(value.workerdLockSha256, "workerdLockSha256") !==
    digest("packages/runtime/workerd.lock.json")
  ) {
    throw new Error("workerd lock digest drift");
  }
  const lock = record(
    json("packages/runtime/workerd.lock.json"),
    "workerd lock",
  );
  const workerd = record(value.workerd, "baseline.workerd");
  if (
    lock.release !== workerd.release ||
    lock.revision !== workerd.revision ||
    lock.expectedVersionOutput !==
      string(
        workerd.expectedVersionOutput,
        "baseline.workerd.expectedVersionOutput",
      )
  ) {
    throw new Error("workerd release identity drift");
  }
  const workersTypes = record(value.workersTypes, "workersTypes");
  const lockTypes = record(lock.workersTypes, "lock.workersTypes");
  if (
    workersTypes.version !== "5.20260830.1" ||
    lockTypes.version !== workersTypes.version ||
    lockTypes.gitHead !== workersTypes.gitHead ||
    !/^[0-9a-f]{40}$/.test(string(lockTypes.gitHead, "workersTypes.gitHead")) ||
    workersTypes.lockSha256 !== digest("bun.lock") ||
    workersTypes.packageSha256 !== lockTypes.packageSha256 ||
    workersTypes.astSha256 !== lockTypes.astSha256
  ) {
    throw new Error("workers-types lock identity drift");
  }
  const sdk = record(value.workersSdk, "workersSdk");
  const lockSdk = record(lock.workersSdk, "lock.workersSdk");
  if (
    sdk.revision !== lockSdk.revision ||
    lockSdk.wranglerVersion !== record(value.wrangler, "wrangler").version ||
    lockSdk.vitePluginVersion !==
      record(value.vitePlugin, "vitePlugin").version ||
    !/^[0-9a-f]{40}$/.test(string(sdk.revision, "workersSdk.revision")) ||
    !/^[0-9a-f]{64}$/.test(string(sdk.lockSha256, "workersSdk.lockSha256"))
  ) {
    throw new Error("workers-sdk identity is not immutable");
  }
  const docs = record(value.cloudflareDocs, "cloudflareDocs");
  if (
    !/^[0-9a-f]{40}$/.test(string(docs.revision, "cloudflareDocs.revision")) ||
    !/^[0-9a-f]{64}$/.test(string(docs.treeSha256, "cloudflareDocs.treeSha256"))
  ) {
    throw new Error("Cloudflare docs identity is not immutable");
  }
}

export function catalogSchema(): void {
  const value = catalog();
  if (value.schemaVersion !== 1) throw new Error("unsupported catalog schema");
  const ids = new Set<string>();
  for (const contract of contracts()) {
    const id = string(contract.id, "contract.id");
    if (!/^[a-z0-9]+(?:[.-][a-z0-9]+)*$/.test(id) || ids.has(id))
      throw new Error(`duplicate or invalid contract id: ${id}`);
    ids.add(id);
    const status = string(contract.status, `${id}.status`);
    if (
      ![
        "supported",
        "supported_with_deviation",
        "unsupported",
        "blocked",
      ].includes(status)
    ) {
      throw new Error(`${id}: invalid status`);
    }
    if ("methods" in contract)
      throw new Error(`${id}: coarse methods lists are forbidden`);
    const positive = strings(contract.positiveCases, `${id}.positiveCases`);
    const negative = strings(contract.negativeCases, `${id}.negativeCases`);
    if (
      (status === "supported" || status === "supported_with_deviation") &&
      (!positive.length || !negative.length)
    )
      throw new Error(
        `${id}: supported contract lacks positive or negative evidence`,
      );
    if (!array(contract.sources, `${id}.sources`).length)
      throw new Error(`${id}: source is missing`);
    for (const raw of array(contract.sources, `${id}.sources`)) {
      const source = record(raw, `${id}.source`);
      const revision = string(source.revision, `${id}.source.revision`);
      const sourcePath = string(source.path, `${id}.source.path`);
      if (
        !/^[0-9a-f]{40}$/.test(revision) ||
        sourcePath.startsWith("/") ||
        sourcePath.includes("..") ||
        !/^[0-9a-f]{64}$/.test(string(source.sha256, `${id}.source.sha256`))
      ) {
        throw new Error(`${id}: source identity is not immutable`);
      }
      if (source.kind === "cloudflare-doc") {
        const url = string(source.url, `${id}.source.url`);
        if (!url.includes(`/blob/${revision}/${sourcePath}`))
          throw new Error(
            `${id}: Cloudflare source URL is not revision-pinned`,
          );
      }
    }
  }
  const evidenceIds = new Set<string>();
  for (const [index, raw] of array(
    catalog().memberEvidence ?? [],
    "memberEvidence",
  ).entries()) {
    const item = record(raw, `memberEvidence[${index}]`);
    const id = string(item.id, `memberEvidence[${index}].id`);
    if (evidenceIds.has(id))
      throw new Error(`duplicate memberEvidence id: ${id}`);
    evidenceIds.add(id);
    const status = string(item.status, `${id}.status`);
    if (
      !["supported", "supported_with_deviation", "blocked"].includes(status)
    ) {
      throw new Error(`${id}: invalid memberEvidence status`);
    }
    const compileCases = strings(item.compileCases ?? [], `${id}.compileCases`);
    const runtimeCases = strings(item.runtimeCases ?? [], `${id}.runtimeCases`);
    if (
      (status === "supported" || status === "supported_with_deviation") &&
      (!compileCases.length || !runtimeCases.length)
    ) {
      throw new Error(
        `${id}: supported member lacks compile and real-runtime cases`,
      );
    }
    if (status === "blocked" && (compileCases.length || runtimeCases.length)) {
      throw new Error(`${id}: blocked member must not carry evidence cases`);
    }
  }
  for (const [index, raw] of array(
    catalog().blockedGaps ?? [],
    "blockedGaps",
  ).entries()) {
    const gap = record(raw, `blockedGaps[${index}]`);
    string(gap.id, `blockedGaps[${index}].id`);
    const memberIds = strings(gap.memberIds, `blockedGaps[${index}].memberIds`);
    if (!memberIds.length || new Set(memberIds).size !== memberIds.length) {
      throw new Error(
        `${gap.id}: blocked gap has no exact member IDs or contains duplicates`,
      );
    }
  }
}

export function capabilityCatalogBijection(): void {
  const advertised = capabilities();
  const mapped = new Map<string, JsonRecord>();
  for (const contract of contracts()) {
    for (const product of productNames(contract)) {
      if (mapped.has(product))
        throw new Error(`product ${product} has more than one catalog owner`);
      mapped.set(product, contract);
    }
  }
  if (
    Object.keys(advertised).sort().join("\0") !==
    [...mapped.keys()].sort().join("\0")
  ) {
    throw new Error("capability and catalog product inventories differ");
  }
  for (const [product, raw] of Object.entries(advertised)) {
    const capability = record(raw, `capability ${product}`);
    const contract = mapped.get(product);
    if (contract === undefined || capability.status !== contract.status)
      throw new Error(`${product}: status differs`);
    if ("methods" in capability)
      throw new Error(`${product}: coarse methods lists are forbidden`);
    const capabilityDeviations = strings(
      capability.deviations ?? [],
      `${product}.deviations`,
    ).sort();
    const contractDeviations =
      product === contract.product
        ? strings(contract.deviations, `${product}.contractDeviations`).sort()
        : [];
    if (capabilityDeviations.join("\0") !== contractDeviations.join("\0"))
      throw new Error(`${product}: deviations differ`);
  }
}
