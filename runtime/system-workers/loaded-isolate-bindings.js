// Assemble only capabilities resolved and verified by RuntimeSource.
import { bindingError } from "./loader-host.js";

function trustedBindingProps(descriptor, deploymentId, routeGeneration, accountId, workerId, durableObject) {
  if (descriptor.kind === "workflow") {
    return Object.freeze({ bindingId: descriptor.bindingId, deploymentId,
      descriptorSha256: descriptor.descriptorSha256, durableObject });
  }
  if (descriptor.kind === "queue_producer") {
    return Object.freeze({
      accountId,
      workerId,
      bindingId: descriptor.bindingId,
      deploymentId,
      descriptorSha256: descriptor.descriptorSha256,
      queueId: descriptor.queueId,
      queueLifecycleGeneration: descriptor.queueLifecycleGeneration,
    });
  }
  return Object.freeze({
    accountId,
    workerId,
    bindingId: descriptor.bindingId,
    deploymentId,
    descriptorSha256: descriptor.descriptorSha256,
    routeGeneration,
    namespaceResourceId: descriptor.resourceId,
    resourceSpecGeneration: descriptor.resourceSpecGeneration,
    permissions: Object.freeze({
      read: descriptor.permissions.read === true,
      write: descriptor.permissions.write === true,
    }),
  });
}

function makeBinding(ctx, descriptor, deploymentId, routeGeneration, accountId, workerId, policy, durableObject) {
  const capability = `${descriptor.kind}@${descriptor.capabilityVersion}`;
  const props = trustedBindingProps(
    descriptor,
    deploymentId,
    routeGeneration,
    accountId,
    workerId,
    durableObject,
  );
  switch (capability) {
    case "kv_namespace@1":
      return ctx.exports.KVNamespace({
        props,
      });
    case "r2_bucket@1":
      return ctx.exports.R2Transport({
        props,
      });
    case "d1_database@1":
      return ctx.exports.D1Transport({
        props,
      });
    case "queue_producer@1":
      return ctx.exports.QueueTransport({ props });
    case "workflow@2":
      return ctx.exports.WorkflowBindingTransportV2({ props });
    case "workflow@1":
      return ctx.exports.WorkflowBindingTransport({ props });
    case "do_namespace@1": {
      if (!/^[0-9a-f]{16}$/.test(descriptor.namespacePrefix)
          || typeof descriptor.namespaceNameKey !== "string") {
        throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
      }
      return Object.freeze({
        schemaVersion: 1,
        namespacePrefix: descriptor.namespacePrefix,
        namespaceNameKey: descriptor.namespaceNameKey,
        maxObjectNameBytes: policy.maxObjectNameBytes,
        transport: ctx.exports.DoTransport({ props }),
      });
    }
    default:
      throw bindingError("BINDING_CAPABILITY_UNSUPPORTED");
  }
}

export function tenantEnv(snapshot, ctx, deploymentId, policy, durableObject = false) {
  const env = { ...snapshot.env };
  const [accountId, workerId] = snapshot.loaderKey.split("/");
  for (const descriptor of snapshot.bindings || []) {
    if (Object.prototype.hasOwnProperty.call(env, descriptor.name)) {
      throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
    }
    env[descriptor.name] = makeBinding(
      ctx,
      descriptor,
      deploymentId,
      snapshot.routeGeneration,
      accountId,
      workerId,
      policy,
      durableObject,
    );
  }
  return env;
}

