// Generated from runtime/src/loader/bindings.ts by Rolldown. Do not edit.
// Assemble only capabilities resolved and verified by RuntimeSource.
import { bindingError } from "./host.js";
function makeBinding(ctx, descriptor, deploymentId, routeGeneration, accountId, workerId, policy, durableObject) {
	const identity = {
		bindingId: descriptor.bindingId,
		deploymentId,
		descriptorSha256: descriptor.descriptorSha256
	};
	if (descriptor.kind === "workflow") {
		const props = Object.freeze({
			...identity,
			durableObject
		});
		if (descriptor.capabilityVersion === 2) return ctx.exports.WorkflowBindingTransportV2({ props });
		if (descriptor.capabilityVersion === 1) return ctx.exports.WorkflowBindingTransport({ props });
		throw bindingError("BINDING_CAPABILITY_UNSUPPORTED");
	}
	if (descriptor.capabilityVersion !== 1) throw bindingError("BINDING_CAPABILITY_UNSUPPORTED");
	if (descriptor.kind === "queue_producer") {
		return ctx.exports.QueueTransport({ props: Object.freeze({
			...identity,
			accountId,
			workerId,
			queueId: descriptor.queueId,
			queueLifecycleGeneration: descriptor.queueLifecycleGeneration
		}) });
	}
	const props = Object.freeze({
		...identity,
		accountId,
		workerId,
		routeGeneration,
		namespaceResourceId: descriptor.resourceId,
		resourceSpecGeneration: descriptor.resourceSpecGeneration,
		permissions: Object.freeze({
			read: descriptor.permissions.read === true,
			write: descriptor.permissions.write === true
		})
	});
	switch (descriptor.kind) {
		case "kv_namespace": return ctx.exports.KVNamespace({ props });
		case "r2_bucket": return ctx.exports.R2Transport({ props });
		case "d1_database": return ctx.exports.D1Transport({ props });
		case "do_namespace": {
			if (typeof descriptor.namespacePrefix !== "string" || !/^[0-9a-f]{16}$/.test(descriptor.namespacePrefix) || typeof descriptor.namespaceNameKey !== "string") throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
			return Object.freeze({
				schemaVersion: 1,
				namespacePrefix: descriptor.namespacePrefix,
				namespaceNameKey: descriptor.namespaceNameKey,
				maxObjectNameBytes: policy.maxObjectNameBytes,
				transport: ctx.exports.DoTransport({ props })
			});
		}
	}
}
export function tenantEnv(snapshot, ctx, deploymentId, policy, durableObject = false) {
	const env = { ...snapshot.env };
	const [accountId, workerId] = snapshot.loaderKey.split("/");
	if (!accountId || !workerId) throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
	for (const descriptor of snapshot.bindings) {
		if (Object.prototype.hasOwnProperty.call(env, descriptor.name)) throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
		env[descriptor.name] = makeBinding(ctx, descriptor, deploymentId, snapshot.routeGeneration, accountId, workerId, policy, durableObject);
	}
	return env;
}
