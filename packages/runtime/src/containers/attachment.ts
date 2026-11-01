import type { TenantDoAuthority } from "../durable-objects/protocol.js";
import type {
  LoaderEnv,
  NativeContainerLease,
  NativeContainerSnapshot,
  NativeHostContainer,
  RuntimeContainerClass,
} from "../loader/protocol.js";
import { bindingError, currentStartupGeneration } from "../loader/shared.js";

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function containerDirectoryPath(value: unknown): string {
  if (
    typeof value !== "string" ||
    !value.startsWith("/") ||
    value.includes("\0") ||
    new TextEncoder().encode(value).length > 4096
  )
    throw bindingError("VERSION_INVARIANT_VIOLATION");
  const parts: string[] = [];
  for (const part of value.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") {
      if (parts.pop() === undefined)
        throw bindingError("VERSION_INVARIANT_VIOLATION");
    } else parts.push(part);
  }
  return `/${parts.join("/")}`;
}

function containerInspectionFromWire(
  value: unknown,
): NativeContainerLease["inspection"] {
  if (
    !record(value) ||
    Object.keys(value).some((key) => key !== "image" && key !== "labels") ||
    typeof value.image !== "string" ||
    value.image.length > 1024 ||
    /[^\x21-\x7e]/.test(value.image) ||
    !Array.isArray(value.labels) ||
    value.labels.length > 10
  )
    throw bindingError("VERSION_INVARIANT_VIOLATION");
  const names = new Set<string>();
  const labels = value.labels.map((label: unknown) => {
    if (
      !record(label) ||
      Object.keys(label).some((key) => key !== "name" && key !== "value") ||
      typeof label.name !== "string" ||
      label.name.length === 0 ||
      new TextEncoder().encode(label.name).length > 16 ||
      /[\x00-\x1f]/.test(label.name) ||
      label.name.startsWith("dev.open-compute.") ||
      names.has(label.name) ||
      typeof label.value !== "string" ||
      new TextEncoder().encode(label.value).length > 64 ||
      /[\x00-\x1f]/.test(label.value)
    )
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    names.add(label.name);
    return { name: label.name, value: label.value };
  });
  return { image: value.image, labels };
}

/** Admit only snapshot metadata; private archive and execution identity stay in the daemon. */
export function containerSnapshotFromWire(
  value: unknown,
  directory?: string,
): NativeContainerSnapshot {
  if (!record(value) || !record(value.snapshot))
    throw bindingError("VERSION_INVARIANT_VIOLATION");
  const snapshot = value.snapshot;
  if (
    Object.keys(snapshot).some(
      (key) => !["id", "size", "name", "dir"].includes(key),
    ) ||
    typeof snapshot.id !== "string" ||
    snapshot.id.length === 0 ||
    snapshot.id.length > 256 ||
    snapshot.id.includes("\0") ||
    !Number.isSafeInteger(snapshot.size) ||
    Number(snapshot.size) < 0 ||
    (snapshot.name !== undefined && typeof snapshot.name !== "string") ||
    (directory === undefined
      ? Object.hasOwn(snapshot, "dir")
      : snapshot.dir !== directory)
  )
    throw bindingError("VERSION_INVARIANT_VIOLATION");
  return {
    id: snapshot.id,
    size: Number(snapshot.size),
    ...(snapshot.name === undefined ? {} : { name: snapshot.name }),
    ...(directory === undefined ? {} : { dir: directory }),
  };
}

/** Validate the private authority response before it reaches the native engine client. */
export function containerLeaseFromWire(
  value: unknown,
): NativeContainerLease | undefined {
  if (!record(value) || !Object.hasOwn(value, "lease"))
    throw bindingError("VERSION_INVARIANT_VIOLATION");
  if (value.lease === null) return undefined;
  const lease = value.lease;
  if (
    !record(lease) ||
    typeof lease.opaqueEngineKey !== "string" ||
    !/^[0-9a-f]{64}$/.test(lease.opaqueEngineKey) ||
    typeof lease.imageRef !== "string" ||
    !/^sha256:[0-9a-f]{64}$/.test(lease.imageRef) ||
    typeof lease.providerContractSha256 !== "string" ||
    !/^[0-9a-f]{64}$/.test(lease.providerContractSha256) ||
    typeof lease.controlToken !== "string" ||
    !/^[0-9a-f]{64}$/.test(lease.controlToken) ||
    typeof lease.callbackToken !== "string" ||
    !/^[0-9a-f]{64}$/.test(lease.callbackToken) ||
    lease.controlToken === lease.callbackToken ||
    typeof lease.nativeStartupJson !== "string" ||
    lease.nativeStartupJson.length > 1048576 ||
    !Number.isSafeInteger(lease.leaseGeneration) ||
    Number(lease.leaseGeneration) < 1 ||
    !Number.isSafeInteger(lease.targetGeneration) ||
    Number(lease.targetGeneration) < 1 ||
    typeof lease.state !== "string" ||
    !["admitting", "starting", "running", "stopping", "lost"].includes(
      lease.state,
    ) ||
    (lease.stopRequestedAtMs !== null &&
      !Number.isSafeInteger(lease.stopRequestedAtMs))
  )
    throw bindingError("VERSION_INVARIANT_VIOLATION");
  return {
    leaseKey: lease.opaqueEngineKey,
    imageRef: lease.imageRef,
    providerContractSha256: lease.providerContractSha256,
    controlToken: lease.controlToken,
    callbackToken: lease.callbackToken,
    nativeStartupJson: lease.nativeStartupJson,
    inspection: containerInspectionFromWire(lease.inspection),
  };
}

/** Extract closed native startup input for lease authority; it encrypts runtime environment values. */
export function containerStartupFromWire(
  json: string,
): Record<string, unknown> | undefined {
  if (json === "") return undefined;
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch {
    throw bindingError("VERSION_INVARIANT_VIOLATION");
  }
  if (!record(value)) throw bindingError("VERSION_INVARIANT_VIOLATION");
  if (
    value.enableInternet !== undefined &&
    typeof value.enableInternet !== "boolean"
  )
    throw bindingError("VERSION_INVARIANT_VIOLATION");
  const startup: Record<string, unknown> = {
    enableInternet: value.enableInternet ?? false,
  };
  if (value.source !== undefined) {
    if (!record(value.source))
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    if (value.source.image !== undefined) {
      if (
        typeof value.source.image !== "string" ||
        value.source.image.length === 0
      )
        throw bindingError("VERSION_INVARIANT_VIOLATION");
      startup.image = value.source.image;
    }
    if (value.source.containerSnapshotId !== undefined) {
      if (
        typeof value.source.containerSnapshotId !== "string" ||
        !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(
          value.source.containerSnapshotId,
        )
      )
        throw bindingError("VERSION_INVARIANT_VIOLATION");
      startup.containerSnapshotId = value.source.containerSnapshotId;
    }
  }
  if (value.instance !== undefined) {
    if (!record(value.instance))
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    if (value.instance.named !== undefined) {
      if (typeof value.instance.named !== "string")
        throw bindingError("VERSION_INVARIANT_VIOLATION");
      startup.instanceProfile = value.instance.named;
    } else if (value.instance.custom !== undefined) {
      const resources = value.instance.custom;
      if (
        !record(resources) ||
        typeof resources.vcpu !== "number" ||
        !Number.isFinite(resources.vcpu) ||
        resources.vcpu <= 0
      )
        throw bindingError("VERSION_INVARIANT_VIOLATION");
      // Cap'n Proto encodes UInt64 as decimal JSON strings.
      const memoryMib =
        typeof resources.memoryMib === "string"
          ? Number(resources.memoryMib)
          : resources.memoryMib;
      const diskMb =
        typeof resources.diskMb === "string"
          ? Number(resources.diskMb)
          : resources.diskMb;
      if (
        !Number.isSafeInteger(memoryMib) ||
        Number(memoryMib) <= 0 ||
        !Number.isSafeInteger(diskMb) ||
        Number(diskMb) <= 0
      )
        throw bindingError("VERSION_INVARIANT_VIOLATION");
      startup.resources = { vcpu: resources.vcpu, memoryMib, diskMb };
    } else throw bindingError("VERSION_INVARIANT_VIOLATION");
  }
  for (const field of ["entrypoint", "environmentVariables"] as const) {
    if (value[field] !== undefined) {
      const values = value[field];
      if (
        !Array.isArray(values) ||
        values.length > 1000 ||
        values.some(
          (item: unknown) => typeof item !== "string" || item.includes("\0"),
        )
      )
        throw bindingError("VERSION_INVARIANT_VIOLATION");
      startup[field] = values;
    }
  }
  if (value.labels !== undefined) {
    if (
      !Array.isArray(value.labels) ||
      value.labels.length > 100 ||
      value.labels.some(
        (label: unknown) =>
          !record(label) ||
          typeof label.name !== "string" ||
          typeof label.value !== "string",
      )
    )
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    startup.labels = value.labels;
  }
  if (value.directorySnapshots !== undefined) {
    if (!Array.isArray(value.directorySnapshots))
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    startup.directorySnapshots = value.directorySnapshots.map(
      (entry: unknown) => {
        if (
          !record(entry) ||
          (entry.snapshotId !== undefined &&
            (typeof entry.snapshotId !== "string" ||
              !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(
                entry.snapshotId,
              )))
        )
          throw bindingError("VERSION_INVARIANT_VIOLATION");
        const mountPoint = containerDirectoryPath(entry.restorePath);
        if (mountPoint === "/")
          throw bindingError("VERSION_INVARIANT_VIOLATION");
        return {
          snapshotId: entry.snapshotId ?? null,
          mountPoint,
        };
      },
    );
  }
  return startup;
}

/** Mint a class-scoped native grant; mutable targets and leases never enter RuntimeSnapshot. */
export function containerAttachment(
  env: LoaderEnv,
  ctx: DurableObjectState,
  authority: TenantDoAuthority,
  declaration: RuntimeContainerClass,
): NativeHostContainer {
  if (
    declaration.className !== authority.className ||
    declaration.namespaceResourceId !== authority.namespaceResourceId
  )
    throw bindingError("VERSION_INVARIANT_VIOLATION");
  const frozen = Object.freeze({
    instanceId: authority.instanceId,
    namespaceResourceId: authority.namespaceResourceId,
    applicationName: declaration.applicationName,
    objectId: authority.objectId,
    objectGeneration: authority.objectGeneration,
    routeGeneration: authority.routeGeneration,
    descriptorSha256: declaration.descriptorSha256,
  });
  let nativeLease: NativeContainerLease | undefined;
  let stopIdentity:
    { leaseGeneration: number; opaqueEngineKey: string } | undefined;
  async function request(
    path: "resolve" | "begin-stop" | "stop" | "snapshot",
    body: unknown,
  ): Promise<Response> {
    const response = await env.RUNTIME_SOURCE.fetch(
      `http://runtime-source/internal/runtime/v1/containers/${path}`,
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-open-compute-startup-generation": currentStartupGeneration(),
        },
        body: JSON.stringify(body),
      },
    );
    if (!response.ok) throw bindingError("CONTAINER_UNAVAILABLE");
    return response;
  }
  async function readLease(
    start: boolean,
    forStop: boolean,
    startup?: Record<string, unknown>,
    expectedLeaseKey = "",
    forMonitor = false,
  ): Promise<NativeContainerLease | undefined> {
    const response = await request("resolve", {
      ...frozen,
      start,
      forStop,
      forMonitor,
      ...(startup === undefined ? {} : { startup }),
    });
    const body: unknown = await response.json();
    nativeLease = containerLeaseFromWire(body);
    if (
      expectedLeaseKey !== "" &&
      ((nativeLease !== undefined &&
        nativeLease.leaseKey !== expectedLeaseKey) ||
        (nativeLease === undefined && !forStop))
    )
      throw bindingError("CONTAINER_UNAVAILABLE");
    if (
      nativeLease !== undefined &&
      !forStop &&
      record(body) &&
      record(body.lease) &&
      (body.lease.stopRequestedAtMs !== null ||
        (start
          ? !["starting", "running"].includes(String(body.lease.state))
          : forMonitor
            ? !["starting", "running", "lost"].includes(
                String(body.lease.state),
              )
            : body.lease.state !== "running"))
    )
      throw bindingError("CONTAINER_UNAVAILABLE");
    if (nativeLease === undefined) stopIdentity = undefined;
    else {
      if (
        !record(body) ||
        !record(body.lease) ||
        typeof body.lease.leaseGeneration !== "number"
      )
        throw bindingError("VERSION_INVARIANT_VIOLATION");
      stopIdentity = {
        leaseGeneration: body.lease.leaseGeneration,
        opaqueEngineKey: nativeLease.leaseKey,
      };
    }
    return nativeLease;
  }
  return env.WORKER_LOADER_FACTORY.getContainer(
    ctx.facets,
    declaration.className,
    async (start, stop, completed, startup, expectedLeaseKey, monitor) => {
      if (
        typeof monitor !== "boolean" ||
        (monitor && (start || stop || completed || startup !== "")) ||
        typeof expectedLeaseKey !== "string" ||
        (expectedLeaseKey !== "" && !/^[0-9a-f]{64}$/.test(expectedLeaseKey))
      )
        throw bindingError("VERSION_INVARIANT_VIOLATION");
      if (!stop)
        return readLease(
          start,
          false,
          start ? containerStartupFromWire(startup) : undefined,
          expectedLeaseKey,
          monitor,
        );
      if (!completed && stopIdentity === undefined)
        await readLease(false, true, undefined, expectedLeaseKey);
      if (
        stopIdentity !== undefined &&
        expectedLeaseKey !== "" &&
        stopIdentity.opaqueEngineKey !== expectedLeaseKey
      )
        throw bindingError("CONTAINER_UNAVAILABLE");
      if (stopIdentity !== undefined) {
        await request(completed ? "stop" : "begin-stop", {
          ...frozen,
          ...stopIdentity,
        });
      }
      if (completed) {
        stopIdentity = undefined;
        nativeLease = undefined;
      }
      return nativeLease;
    },
    async (expectedLeaseKey, name, directory) => {
      const path =
        directory === undefined ? undefined : containerDirectoryPath(directory);
      if (!/^[0-9a-f]{64}$/.test(expectedLeaseKey))
        throw bindingError("VERSION_INVARIANT_VIOLATION");
      await readLease(false, false, undefined, expectedLeaseKey);
      if (
        stopIdentity === undefined ||
        stopIdentity.opaqueEngineKey !== expectedLeaseKey
      )
        throw bindingError("CONTAINER_UNAVAILABLE");
      const response = await request("snapshot", {
        source: { ...frozen, ...stopIdentity },
        ...(name === undefined ? {} : { name }),
        ...(path === undefined ? {} : { directory: path }),
      });
      const body: unknown = await response.json();
      return containerSnapshotFromWire(body, path);
    },
    declaration.images,
  );
}
