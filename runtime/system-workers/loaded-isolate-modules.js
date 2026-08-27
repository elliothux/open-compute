// Immutable dynamic-module assembly shared by fetch, DO, and Workflow hosts.
import r2FacadeSource from "r2-facade-source";
import d1FacadeSource from "d1-facade-source";
import doFacadeSource from "do-facade-source";
import doIdCodecSource from "do-id-codec-source";
import doAlarmShimSource from "do-alarm-shim-source";
import queueFacadeSource from "queue-facade-source";
import workflowRunnerSource from "workflow-runner-source";
import workflowJsonSource from "workflow-json-source";
import workflowFacadeSource from "workflow-facade-source";
import {
  DO_ALARM_SHIM_MODULE,
  D1_FACADE_MODULE,
  DO_FACADE_MODULE,
  DO_ID_CODEC_MODULE,
  LOADED_ISOLATE_RESERVED_MODULES,
  LOADED_ISOLATE_WRAPPER_MODULE,
  QUEUE_FACADE_MODULE,
  R2_FACADE_MODULE,
  WORKFLOW_JSON_MODULE,
  WORKFLOW_FACADE_MODULE,
  generateBindingWrapper as generateWorkflowBindingWrapper,
} from "./loaded-isolate-wrapper-generator-v2.js";
import { generateBindingWrapper } from "./loaded-isolate-wrapper-generator.js";
import { bindingError } from "./loader-host.js";

export function bytes(base64) {
  const binary = atob(base64);
  const value = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) value[i] = binary.charCodeAt(i);
  return value;
}

function moduleValue(module) {
  const raw = bytes(module.bytesBase64);
  switch (module.type) {
    case "esModule":
      return { js: new TextDecoder("utf-8", { fatal: true }).decode(raw) };
    case "commonJsModule":
      return { cjs: new TextDecoder("utf-8", { fatal: true }).decode(raw) };
    case "text":
      return { text: new TextDecoder("utf-8", { fatal: true }).decode(raw) };
    case "json":
      return { json: JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(raw)) };
    case "data":
      return { data: raw };
    case "wasm":
      return { wasm: raw };
    default:
      throw new Error("unsupported module representation");
  }
}

export function modulesFor(snapshot, validation, entrypointName, durableObject = false, workflow = false) {
  const modules = {};
  for (const module of snapshot.modules) modules[module.name] = moduleValue(module);
  const r2Bindings = (snapshot.bindings || [])
    .filter((binding) => binding.kind === "r2_bucket" && binding.capabilityVersion === 1)
    .map((binding) => binding.name);
  const d1Bindings = (snapshot.bindings || [])
    .filter((binding) => binding.kind === "d1_database" && binding.capabilityVersion === 1)
    .map((binding) => binding.name);
  const doBindings = (snapshot.bindings || [])
    .filter((binding) => binding.kind === "do_namespace" && binding.capabilityVersion === 1)
    .map((binding) => binding.name);
  const queueBindings = (snapshot.bindings || [])
    .filter((binding) => binding.kind === "queue_producer" && binding.capabilityVersion === 1)
    .map((binding) => binding.name);
  const workflowBindings = (snapshot.bindings || [])
    .filter((binding) => binding.kind === "workflow" && binding.capabilityVersion === 1)
    .map((binding) => binding.name);
  for (const reserved of LOADED_ISOLATE_RESERVED_MODULES) {
    if (Object.prototype.hasOwnProperty.call(modules, reserved)) {
      throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
    }
  }
  if (r2Bindings.length) modules[R2_FACADE_MODULE] = { js: r2FacadeSource };
  if (d1Bindings.length) modules[D1_FACADE_MODULE] = { js: d1FacadeSource };
  if (doBindings.length) {
    modules[DO_ID_CODEC_MODULE] = { js: doIdCodecSource };
    modules[DO_FACADE_MODULE] = { js: doFacadeSource };
  }
  if (queueBindings.length) modules[QUEUE_FACADE_MODULE] = { js: queueFacadeSource };
  if (workflowBindings.length) modules[WORKFLOW_FACADE_MODULE] = { js: workflowFacadeSource };
  if (workflow || workflowBindings.length) {
    modules[WORKFLOW_JSON_MODULE] = { js: workflowJsonSource };
  }
  if (entrypointName && durableObject) {
    modules[DO_ALARM_SHIM_MODULE] = { js: doAlarmShimSource };
  }
  modules[LOADED_ISOLATE_WRAPPER_MODULE] = {
    js: (workflow || workflowBindings.length ? generateWorkflowBindingWrapper : generateBindingWrapper)(
      snapshot.mainModule,
      r2Bindings,
      d1Bindings,
      doBindings,
      entrypointName,
      durableObject,
      queueBindings,
      workflow,
      workflowBindings,
      workflowRunnerSource,
    ),
  };
  let mainModule = LOADED_ISOLATE_WRAPPER_MODULE;
  if (validation) {
    const wrapper = "__open_compute_validation__.js";
    const exportName = entrypointName || "default";
    modules[wrapper] = { js: `import * as tenant from ${JSON.stringify(`./${mainModule}`)};\nif (!(${JSON.stringify(exportName)} in tenant)) throw new Error(\"missing entrypoint\");\nexport default { fetch() { return new Response(\"open-compute-validation-v1\"); } };` };
    return { modules, mainModule: wrapper };
  }
  return { modules, mainModule };
}
