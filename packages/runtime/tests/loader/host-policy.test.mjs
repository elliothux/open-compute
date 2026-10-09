import assert from "node:assert/strict";
import test from "node:test";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const policyModule = moduleUrl(await compileRuntime("loader/policy.ts"));
const { workerPolicy, PRIVATE_POLICY } = await import(policyModule);
const calls = [];
globalThis.__hostPolicyCalls = calls;
const factories = moduleUrl(`
  const calls = globalThis.__hostPolicyCalls;
  export function createServiceBinding(env, createStub, control, admit) {
    calls.push(['service-policy', control, admit]);
    return createStub(env);
  }
  export default { createServiceRpcStub(policy) { return { native: policy }; },
    createPrivateTransport(raw) {
      const control = { control: raw };
      calls.push(['private-control', raw, control]);
      return control;
    },
    admitSubrequest() { calls.push(['admit']); },
    createDurableObjectId(value, name, jurisdiction) { return { value, name, jurisdiction }; },
    createDurableObjectNamespace(policy) { return policy; },
    createDurableObjectStub(id, raw, policy) { return { id, raw, policy }; },
    isRpcStub() { return false; },
    installQueuePolicy(queue, policy) { calls.push(["install-queue", queue, policy]); }, decodeQueueV8() {} };
  export function registerServiceBinding() {}
  export function createDurableObjectNamespace(...args) { calls.push(['do-binding',...args]); return {}; }
  export function createDurableObjectStubPolicy(env, factories) { return factories.createRpcStub(env); }
  export class QueuePublisher { constructor(binding, ...args) { calls.push(["queue",...args]); }
    send() {} sendBatch() {} metrics() {} }
  export function registerOutputPublisher(...args) { calls.push(["publisher", ...args]); }
  export class WorkflowImpl { constructor(...args) { calls.push(['workflow-binding',...args]); } }
  export function triggerWorkflowSchedule() {}
  export function runWorkflow() {}
  export function validateWorkflowClass() { return true; }
  export function registerForwarding(...args) { calls.push(['forwarding',...args]); }
  export function createCacheRuntime(...args) { return { args, bind() {} }; }
  export function wrapDefault(...args) { calls.push(['default',...args]); return { kind: 'default', args }; }
  export function wrapDefaultService(...args) { return { kind: 'service-default', args }; }
  export function wrapEntrypoint(...args) { return { kind: 'entrypoint', args }; }
  export function wrapDurableObject(...args) { return { kind: 'do', args }; }
  export function createWorkflowEntrypoint(...args) { return { kind: 'workflow', args }; }
  export function createLoopbackEntrypoint(...args) { return { kind: 'loopback', args }; }
  export function validationHandler(tenant, name, requireConstructor) {
    if (requireConstructor && (typeof tenant[name] !== "function" || !tenant[name].prototype)) throw Error("missing entrypoint");
    if (!Object.hasOwn(tenant, name)) throw Error('missing entrypoint');
    return { kind: 'validation', name };
  }
`);
const imports = Object.fromEntries(
  [
    "cloudflare-internal:open-compute-forwarding",
    "cloudflare-internal:wrapped-binding",
    "../cache/facade.js",
    "../durable-objects/namespace.js",
    "../durable-objects/stub.js",
    "../queues/publisher.js",
    "../durable-objects/output-gate.js",
    "../services/facade.js",
    "../workflows/facade.js",
    "../workflows/runner.js",
    "./wrappers/durable-object.js",
    "./wrappers/loopback.js",
    "./wrappers/runtime.js",
    "./wrappers/workflow.js",
  ].map((name) => [name, factories]),
);
const {
  default: hostPolicy,
  createServiceBinding,
  createDurableObjectStubPolicy,
} = await importRuntime("loader/host-policy.ts", {
  ...imports,
  "./policy.js": policyModule,
});
test("native Service stub factory is provided by the INTERNAL host closure", () => {
  const env = { fetcher: {} };
  assert.deepEqual(createServiceBinding(env), { native: env });
  const [, control, admit] = calls.find((call) => call[0] === "service-policy");
  assert.equal(control.control, env.fetcher);
  admit();
  assert.equal(calls.at(-1)[0], "admit");
});
test("native DO transfer uses the same INTERNAL capability factory", () => {
  const env = { fetcher: {}, id: "a".repeat(64) };
  assert.deepEqual(createDurableObjectStubPolicy(env), { native: env });
});
const snapshot = {
  contentKind: "worker",
  mainModule: "main.py",
  modules: [],
  loaderKey: "account/worker/version",
  routeGeneration: 3,
  workerCodeSha256: "aa".repeat(32),
  bindings: [],
  services: [],
  scheduledTargets: [],
  workerLoaders: [],
  browserBindings: [],
  cachePolicy: {
    enabled: true,
    failOpen: false,
    entrypoints: { Named: { enabled: true } },
  },
};
function initialize(
  options = {},
  publicEnvironment = {},
  privateEnvironment = {},
) {
  const policy = {
    ...workerPolicy(snapshot, false, undefined, false, false),
    ...options,
  };
  const authority = { ...privateEnvironment, [PRIVATE_POLICY]: policy };
  return {
    transform: hostPolicy(publicEnvironment, authority),
    authority,
    publicEnvironment,
    policy,
  };
}

test("host policy materializes remaining bindings once before tenant initialization", () => {
  calls.length = 0;
  const native = { native: true };
  const raw = { private: true };
  const bindings = [
    "do_namespace",
    "queue_producer",
    "workflow",
    "kv_namespace",
  ].map((kind, index) => ({ kind, name: `B${index}`, capabilityVersion: 1 }));
  const environment = {
    B1: native,
    B3: native,
    SERVICE: native,
    VALUE: "public",
  };
  const { transform, authority } = initialize(
    { bindings, services: [{ name: "SERVICE" }] },
    environment,
    {
      B0: raw,
      B1: raw,
      B2: raw,
      __OPEN_COMPUTE_PRIVATE_CACHE: raw,
    },
  );
  assert.equal(environment.B1, native);
  const doFactory = calls.find((call) => call[0] === "do-binding")[2];
  assert.deepEqual(doFactory.createId("id", "name", "eu"), {
    value: "id",
    name: "name",
    jurisdiction: "eu",
  });
  assert.equal(calls.find((call) => call[0] === "install-queue")[1], native);
  assert.equal(
    typeof calls.find((call) => call[0] === "install-queue")[2].send,
    "function",
  );
  assert.equal(environment.B3, native);
  assert.equal(environment.SERVICE, native);
  assert.equal(environment.VALUE, "public");
  assert.deepEqual(Object.keys(environment).sort(), [
    "B0",
    "B1",
    "B2",
    "B3",
    "SERVICE",
    "VALUE",
  ]);
  assert.equal(calls.filter((call) => call[1] === raw).length, 3);
  assert.equal(environment.__OPEN_COMPUTE_PRIVATE_CACHE, undefined);
  const namespace = { default: {}, Named: class {} };
  transform(namespace);
  transform(namespace);
  assert.equal(calls.filter((call) => call[1] === raw).length, 3);
  assert.equal(calls.find((call) => call[0] === "forwarding")[2], authority);
});

test("host policy transforms JS and official Python named classes without duplicate exports", () => {
  for (const python of [false, true]) {
    const Default = class {};
    const Named = class {};
    const source = python
      ? { default: Default, pythonEntrypoints: { Named } }
      : { default: Default, Named };
    const { transform, authority } = initialize();
    const output = transform(source);
    assert.equal(output.default.kind, "default");
    assert.equal(output.default.args[0], Default);
    const named = python ? output.pythonEntrypoints.Named : output.Named;
    assert.equal(named.kind, "entrypoint");
    assert.equal(named.args[0], Named);
    assert.equal(named.args[2].args[2], authority);
    assert.equal(named.args[2].args[3], "Named");
    assert.equal(output.__OpenComputeDefaultService.kind, "service-default");
    assert.deepEqual(output.__OpenComputeLoopbackService.args[2], ["Named"]);
    assert.equal(source.default, Default);
    if (python) assert.equal(Object.hasOwn(output, "Named"), false);
  }
});

test("selected service, DO and Workflow entrypoints retain their owning private policy", () => {
  for (const options of [
    { entrypointName: "Named", automaticCacheEntrypoints: [] },
    {
      entrypointName: "Named",
      durableObject: true,
      automaticCacheEntrypoints: [],
    },
    {
      entrypointName: "default",
      durableObject: true,
      automaticCacheEntrypoints: [],
    },
    { entrypointName: "Named", workflow: true, automaticCacheEntrypoints: [] },
  ]) {
    const target = class {};
    const { transform, authority } = initialize(options);
    const output = transform({ default: target, Named: target });
    const wrapped = options.workflow
      ? output.__OpenComputeWorkflow
      : output[options.entrypointName];
    assert.equal(
      wrapped.kind,
      options.workflow
        ? "workflow"
        : options.durableObject
          ? "do"
          : "entrypoint",
    );
    if (options.durableObject) assert.equal(wrapped.args[1], authority);
    if (options.durableObject && options.entrypointName === "default")
      assert.equal(output.default, wrapped);
  }
});

test("named-only Service, DO and Workflow modules preserve the absence of a default export", () => {
  for (const options of [
    { entrypointName: "Named" },
    { entrypointName: "Named", durableObject: true },
    { entrypointName: "Named", workflow: true },
  ]) {
    for (const python of [false, true]) {
      const target = class {};
      const { transform } = initialize({
        ...options,
        automaticCacheEntrypoints: [],
      });
      const namespace = python
        ? { pythonEntrypoints: { Named: target } }
        : { Named: target };
      const output = transform(namespace);
      assert.equal(Object.hasOwn(output, "default"), false);
      const selected = options.workflow
        ? output.__OpenComputeWorkflow
        : python
          ? output.pythonEntrypoints.Named
          : output.Named;
      assert.equal(
        selected.kind,
        options.workflow
          ? "workflow"
          : options.durableObject
            ? "do"
            : "entrypoint",
      );
    }
  }
});

test("DO alarm control is private and materialized once before tenant evaluation", () => {
  calls.length = 0;
  const raw = {};
  const publicEnvironment = {};
  const { transform, authority } = initialize(
    { durableObject: true, automaticCacheEntrypoints: [] },
    publicEnvironment,
    { __OPEN_COMPUTE_PRIVATE_ALARM_INDEX: raw },
  );
  const target = class {};
  transform({ default: target });
  transform({ default: target });
  const controls = calls.filter((call) => call[0] === "private-control");
  assert.equal(controls.length, 1);
  assert.equal(controls[0][1], raw);
  assert.equal(authority.__OPEN_COMPUTE_PRIVATE_ALARM_INDEX, controls[0][2]);
  assert.equal(publicEnvironment.__OPEN_COMPUTE_PRIVATE_ALARM_INDEX, undefined);
});

test("DO validation checks JS and Python constructors without wrapping or constructing them", () => {
  let constructed = 0;
  class Named {
    constructor() {
      constructed++;
    }
  }
  for (const name of ["Named", "default", "constructor"]) {
    for (const python of [false, true]) {
      calls.length = 0;
      const { transform } = initialize({
        validation: true,
        durableObject: true,
        entrypointName: name,
        services: [{ name: "UNAVAILABLE" }],
        automaticCacheEnabled: false,
      });
      const namespace = python
        ? { pythonEntrypoints: { [name]: Named } }
        : { [name]: Named };
      assert.deepEqual(transform(namespace).default, {
        kind: "validation",
        name,
      });
      assert.equal(calls.length, 0);
      assert.equal(constructed, 0);
      for (const invalid of [undefined, {}, () => {}]) {
        const exports = invalid === undefined ? {} : { [name]: invalid };
        assert.throws(
          () => transform(python ? { pythonEntrypoints: exports } : exports),
          /missing entrypoint/,
        );
      }
    }
  }
});

test("validation checks official Python exports without constructing request bindings", () => {
  calls.length = 0;
  const { transform } = initialize({
    validation: true,
    entrypointName: "Named",
    services: [{ name: "MISSING" }],
    automaticCacheEnabled: false,
  });
  assert.deepEqual(
    transform({ pythonEntrypoints: { Named: class {} } }).default,
    { kind: "validation", name: "Named" },
  );
  assert.equal(calls.length, 0);
  assert.throws(() => transform({ default: {} }), /missing entrypoint/);
  assert.throws(() => hostPolicy({}, {}), /VERSION_INVARIANT/);
  assert.throws(
    () => hostPolicy({}, { [PRIVATE_POLICY]: [] }),
    /VERSION_INVARIANT/,
  );
});

test("scheduled Workflow policy remains scoped to its verified binding and cron", () => {
  const configured = {
    ...snapshot,
    bindings: [{ name: "FLOW", kind: "workflow", schedules: ["* * * * *"] }],
    scheduledTargets: [
      {
        cron: "* * * * *",
        scheduledHandler: false,
        workflowBindings: ["FLOW"],
      },
    ],
  };
  const policy = workerPolicy(configured, false, undefined, false, false);
  const { transform } = initialize(policy, {}, { FLOW: {} });
  assert.equal(
    transform({ default: {} }).default.args[2].targets,
    policy.scheduledTargets,
  );
  assert.throws(
    () =>
      workerPolicy(
        { ...configured, scheduledTargets: [] },
        false,
        undefined,
        false,
        false,
      ),
    /invalid scheduled/,
  );
  assert.throws(
    () => workerPolicy(snapshot, false, "bad.name", false, false),
    /invalid entrypoint/,
  );
  assert.throws(
    () => workerPolicy(snapshot, false, undefined, true, false),
    /invalid entrypoint/,
  );
});

test("module assembly keeps the original Python main and excludes platform sources", async () => {
  const { modulesFor } = await importRuntime("loader/modules.ts", {
    "./policy.js": policyModule,
    "./module-values.js": moduleUrl(
      await compileRuntime("loader/module-values.ts"),
    ),
    "./shared.js": moduleUrl(
      "export function bindingError(code) { return new Error(code); }",
    ),
  });
  const configured = {
    ...snapshot,
    modules: [
      {
        name: "main.py",
        type: "python",
        bytesBase64: btoa("class Default: pass"),
      },
    ],
  };
  const built = modulesFor(configured, false, undefined);
  assert.equal(built.mainModule, "main.py");
  assert.deepEqual(built.modules, { "main.py": { py: "class Default: pass" } });
  assert.equal(built.policy.validation, false);
  assert.equal(modulesFor(configured, true, "Named").policy.validation, true);
  for (const overrides of [
    { contentKind: "assets" },
    { mainModule: null },
    { bindings: [{ capabilityVersion: 2 }] },
    {
      modules: [
        {
          ...configured.modules[0],
          name: "cloudflare-internal:open-compute-host-policy",
        },
      ],
    },
    {
      modules: [
        { ...configured.modules[0], name: "open-compute:worker-loader" },
      ],
    },
  ])
    assert.throws(
      () => modulesFor({ ...configured, ...overrides }, false, undefined),
      /VERSION_INVARIANT/,
    );
});
