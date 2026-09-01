// Immutable dynamic-module assembly shared by fetch, DO, and Workflow hosts.
import r2FacadeSource from "r2-facade-source";
import kvFacadeSource from "kv-facade-source";
import d1FacadeSource from "d1-facade-source";
import doFacadeSource from "do-facade-source";
import doIdCodecSource from "do-id-codec-source";
import doAlarmShimSource from "do-alarm-shim-source";
import doOutputGateSource from "do-output-gate-source";
import doFacetsSource from "do-facets-source";
import queueFacadeSource from "queue-facade-source";
import serializationCodecSource from "serialization-codec-source";
import serializationEncodeSource from "serialization-encode-source";
import serializationDecodeSource from "serialization-decode-source";
import serializationFormatSource from "serialization-format-source";
import workflowRunnerSource from "workflow-runner-source";
import workflowDurationSource from "workflow-duration-source";
import workflowCodecSource from "workflow-codec-source";
import workflowFacadeSource from "workflow-facade-source";
import wrapperRuntimeSource from "wrapper-runtime-source";
import doWrapperSource from "do-wrapper-source";
import workflowWrapperSource from "workflow-wrapper-source";
import assetFacadeSource from "assets-facade-source";
import serviceFacadeSource from "service-facade-source";
import serviceScopeSource from "service-scope-source";
import socketTunnelSource from "socket-tunnel-source";
import cacheFacadeSource from "cache-facade-source";
import imagesFacadeSource from "images-facade-source";
import {
  ASSET_FACADE_MODULE, CACHE_FACADE_MODULE, D1_FACADE_MODULE, DO_ALARM_SHIM_MODULE, DO_FACADE_MODULE, DO_ID_CODEC_MODULE,
  DO_FACETS_MODULE, DO_OUTPUT_GATE_MODULE,
  DO_WRAPPER_MODULE, INTERNAL_MODULE_PREFIX, LOADED_ISOLATE_WRAPPER_MODULE,
  IMAGES_FACADE_MODULE, KV_FACADE_MODULE, QUEUE_FACADE_MODULE, R2_FACADE_MODULE, SERIALIZATION_CODEC_MODULE,
  SERIALIZATION_DECODE_MODULE, SERIALIZATION_ENCODE_MODULE, SERIALIZATION_FORMAT_MODULE,
  SERVICE_FACADE_MODULE, SERVICE_SCOPE_MODULE,
  SOCKET_TUNNEL_MODULE,
  VALIDATION_MODULE, WORKFLOW_FACADE_MODULE,
  WORKFLOW_CODEC_MODULE, WORKFLOW_DURATION_MODULE, WORKFLOW_RUNNER_MODULE, WORKFLOW_WRAPPER_MODULE,
  WRAPPER_RUNTIME_MODULE, generateBindingWrapper, generateValidationWrapper,
} from "./wrappers/generator.js";
import { bindingError } from "./host.js";
import type { RuntimeModule, RuntimeSnapshot } from "./protocol.js";

export function bytes(base64: string): Uint8Array<ArrayBuffer> {
  const binary = atob(base64);
  const value = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) value[i] = binary.charCodeAt(i);
  return value;
}

function moduleValue(module: RuntimeModule): WorkerLoaderModule {
  const raw = bytes(module.bytesBase64);
  switch (module.type) {
    case "esModule":
      return { js: new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(raw) };
    case "commonJsModule":
      return { cjs: new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(raw) };
    case "text":
      return { text: new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(raw) };
    case "json":
      return { json: JSON.parse(new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(raw)) };
    case "data":
      return { data: raw.buffer };
    case "wasm":
      return { wasm: raw };
    default:
      throw new Error("unsupported module representation");
  }
}

export function modulesFor(snapshot: RuntimeSnapshot, validation: boolean, entrypointName: string | undefined, durableObject = false, workflow = false) {
  if (snapshot.contentKind !== "worker" || typeof snapshot.mainModule !== "string") {
    throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
  }
  const modules: Record<string, WorkerLoaderModule> = {};
  for (const module of snapshot.modules) {
    if (module.name.startsWith(INTERNAL_MODULE_PREFIX)) throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
    Object.defineProperty(modules, module.name, { value: moduleValue(module), enumerable: true });
  }
  const has = (kind: string, version = 1) => snapshot.bindings.some(binding => binding.kind === kind && binding.capabilityVersion === version);
  if (snapshot.bindings.some(binding => binding.capabilityVersion !== 1)) {
    throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
  }
  modules[WRAPPER_RUNTIME_MODULE] = { js: wrapperRuntimeSource };
  modules[SERVICE_SCOPE_MODULE] = { js: serviceScopeSource };
  modules[SOCKET_TUNNEL_MODULE] = { js: socketTunnelSource };
  if (has("kv_namespace")) modules[KV_FACADE_MODULE] = { js: kvFacadeSource };
  if (has("r2_bucket")) modules[R2_FACADE_MODULE] = { js: r2FacadeSource };
  if (has("d1_database")) modules[D1_FACADE_MODULE] = { js: d1FacadeSource };
  if (has("do_namespace")) {
    modules[DO_ID_CODEC_MODULE] = { js: doIdCodecSource };
    modules[DO_FACADE_MODULE] = { js: doFacadeSource };
  }
  if (has("queue_producer") || has("workflow") || durableObject) {
    modules[DO_OUTPUT_GATE_MODULE] = { js: doOutputGateSource };
  }
  if (has("queue_producer") || has("workflow") || workflow) {
    modules[SERIALIZATION_FORMAT_MODULE] = { js: serializationFormatSource };
    modules[SERIALIZATION_ENCODE_MODULE] = { js: serializationEncodeSource };
    modules[SERIALIZATION_DECODE_MODULE] = { js: serializationDecodeSource };
    modules[SERIALIZATION_CODEC_MODULE] = { js: serializationCodecSource };
    modules[QUEUE_FACADE_MODULE] = { js: queueFacadeSource };
  }
  if (has("workflow")) modules[WORKFLOW_FACADE_MODULE] = { js: workflowFacadeSource };
  if (snapshot.assetBinding) modules[ASSET_FACADE_MODULE] = { js: assetFacadeSource };
  const cacheAvailable = !durableObject && !workflow;
  if (cacheAvailable) modules[CACHE_FACADE_MODULE] = { js: cacheFacadeSource };
  if (snapshot.imagesBinding && cacheAvailable) modules[IMAGES_FACADE_MODULE] = { js: imagesFacadeSource };
  modules[SERVICE_FACADE_MODULE] = { js: serviceFacadeSource };
  if (workflow || has("workflow")) modules[WORKFLOW_CODEC_MODULE] = { js: workflowCodecSource };
  if (workflow) {
    modules[WORKFLOW_WRAPPER_MODULE] = { js: workflowWrapperSource };
    modules[WORKFLOW_RUNNER_MODULE] = { js: workflowRunnerSource };
    modules[WORKFLOW_DURATION_MODULE] = { js: workflowDurationSource };
  }
  if (entrypointName && durableObject) {
    modules[DO_ALARM_SHIM_MODULE] = { js: doAlarmShimSource };
    modules[DO_FACETS_MODULE] = { js: doFacetsSource };
    modules[DO_WRAPPER_MODULE] = { js: doWrapperSource };
  }
  modules[LOADED_ISOLATE_WRAPPER_MODULE] = {
    js: generateBindingWrapper({
      mainModule: snapshot.mainModule, bindings: snapshot.bindings, services: snapshot.services,
      entrypointName, durableObject, workflow, assetBindingName: snapshot.assetBinding?.name,
      imagesBindingName: cacheAvailable ? snapshot.imagesBinding?.name : undefined,
      scheduledTargets: snapshot.scheduledTargets,
      cacheAvailable,
      cacheFailOpen: snapshot.cachePolicy.failOpen,
      automaticCacheEntrypoints: entrypointName === undefined
        ? Object.entries(snapshot.cachePolicy.entrypoints)
          .filter(([, policy]) => policy.enabled)
          .map(([name]) => name)
        : [],
      automaticCacheEnabled: !validation && cacheAvailable && (entrypointName === undefined
        ? snapshot.cachePolicy.enabled
        : (snapshot.cachePolicy.entrypoints[entrypointName]?.enabled ?? snapshot.cachePolicy.enabled)),
    }),
  };
  if (validation) {
    modules[VALIDATION_MODULE] = { js: generateValidationWrapper(entrypointName) };
    return { modules, mainModule: VALIDATION_MODULE };
  }
  return { modules, mainModule: LOADED_ISOLATE_WRAPPER_MODULE };
}
