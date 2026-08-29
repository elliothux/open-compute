using Workerd = import "/workerd/workerd.capnp";

const internalToken :Text = "__OPEN_COMPUTE_INTERNAL_TOKEN__";
const bindingToken :Text = "__OPEN_COMPUTE_BINDING_TOKEN__";
const doMaxObjectNameBytes :Text = "__OPEN_COMPUTE_DO_MAX_OBJECT_NAME_BYTES__";
const doMaxRpcRequestBytes :Text = "__OPEN_COMPUTE_DO_MAX_RPC_REQUEST_BYTES__";
const doMaxRpcResponseBytes :Text = "__OPEN_COMPUTE_DO_MAX_RPC_RESPONSE_BYTES__";
const doMaxFetchBodyBytes :Text = "__OPEN_COMPUTE_DO_MAX_FETCH_BODY_BYTES__";
const doDispatchTimeoutMs :Text = "__OPEN_COMPUTE_DO_DISPATCH_TIMEOUT_MS__";
const doMaxInFlightDispatches :Text = "__OPEN_COMPUTE_DO_MAX_IN_FLIGHT_DISPATCHES__";
const doDiskStopWritesPercent :Text = "__OPEN_COMPUTE_DO_DISK_STOP_WRITES_PERCENT__";

const config :Workerd.Config = (
  structuredLogging = true,

  services = [
    (name = "ingress", worker = .ingressWorker),
    (name = "loader-host", worker = .loaderHostWorker),
    (name = "do-router", worker = .doHostWorker),
    (name = "do-storage", disk = (writable = true, allowDotfiles = true)),
    # The address is deliberately omitted from the compiled config. platformd
    # injects a generation-local loopback listener with --external-addr.
    (name = "runtime-source", external = (http = ())),
    (name = "binding-backend", external = (http = ())),
    (name = "outbound-gateway", worker = .outboundGatewayWorker),
    (name = "internet", network = (allow = ["public"])),
  ],

  sockets = [
    (name = "http", address = "127.0.0.1:0", http = (), service = "ingress"),
  ],
);

const ingressWorker :Workerd.Worker = (
  compatibilityDate = "2026-08-22",
  compatibilityFlags = ["nodejs_compat", "rpc", "enable_ctx_exports", "experimental"],
  modules = [
    (name = "gateway/ingress.js", esModule = embed "dist/gateway/ingress.js"),
  ],
  bindings = [
    (name = "INTERNAL_TOKEN", text = .internalToken),
    (name = "LOADER_HOST", service = "loader-host"),
    (name = "DO_ROUTER", service = "do-router"),
  ],
  globalOutbound = "outbound-gateway",
);

const loaderHostWorker :Workerd.Worker = (
  compatibilityDate = "2026-08-22",
  compatibilityFlags = [
    "nodejs_compat",
    "rpc",
    "enable_ctx_exports",
    "experimental",
    "service_binding_extra_handlers",
  ],
  modules = [
    (name = "loader/host.js", esModule = embed "dist/loader/host.js"),
    (name = "loader/shared.js", esModule = embed "dist/loader/shared.js"),
    (name = "services/transport.js", esModule = embed "dist/services/transport.js"),
    (name = "assets/router.js", esModule = embed "dist/assets/router.js"),
    (name = "workflows/host.js", esModule = embed "dist/workflows/host.js"),
    (name = "workflows/controller.js", esModule = embed "dist/workflows/controller.js"),
    (name = "loader/modules.js", esModule = embed "dist/loader/modules.js"),
    (name = "loader/snapshot.js", esModule = embed "dist/loader/snapshot.js"),
    (name = "kv/transport.js", esModule = embed "dist/kv/transport.js"),
    (name = "loader/wrapper-runtime-source", text = embed "dist/loader/wrappers/runtime.js"),
    (name = "loader/do-wrapper-source", text = embed "dist/loader/wrappers/durable-object.js"),
    (name = "loader/workflow-wrapper-source", text = embed "dist/loader/wrappers/workflow.js"),
    (name = "loader/workflow-runner-source", text = embed "dist/workflows/runner.js"),
    (name = "loader/workflow-json-source", text = embed "dist/workflows/json.js"),
    (name = "loader/workflow-facade-source", text = embed "dist/workflows/facade.js"),
    (name = "loader/r2-facade-source", text = embed "dist/r2/facade.js"),
    (name = "loader/d1-facade-source", text = embed "dist/d1/facade.js"),
    (name = "loader/do-facade-source", text = embed "dist/durable-objects/facade.js"),
    (name = "loader/do-id-codec-source", text = embed "dist/durable-objects/id-codec.js"),
    (name = "loader/do-alarm-shim-source", text = embed "dist/durable-objects/alarm-shim.js"),
    (name = "loader/queue-facade-source", text = embed "dist/queues/facade.js"),
    (name = "loader/assets-facade-source", text = embed "dist/assets/facade.js"),
    (name = "loader/service-facade-source", text = embed "dist/services/facade.js"),
    (name = "loader/service-scope-source", text = embed "dist/services/scope.js"),
    (name = "loader/wrappers/generator.js", esModule = embed "dist/loader/wrappers/generator.js"),
    (name = "workflows/binding.js", esModule = embed "dist/workflows/binding.js"),
    (name = "loader/bindings.js", esModule = embed "dist/loader/bindings.js"),
    (name = "r2/transport.js", esModule = embed "dist/r2/transport.js"),
    (name = "d1/transport.js", esModule = embed "dist/d1/transport.js"),
  ],
  bindings = [
    (name = "LOADER", workerLoader = (id = "open-compute")),
    (name = "RUNTIME_SOURCE", service = "runtime-source"),
    (name = "BINDING_BACKEND", service = "binding-backend"),
    (name = "BINDING_BACKEND_TOKEN", text = .bindingToken),
    (name = "INTERNAL_TOKEN", text = .internalToken),
    (name = "DO_ROUTER", service = "do-router"),
    (name = "DO_MAX_OBJECT_NAME_BYTES", text = .doMaxObjectNameBytes),
    (name = "DO_MAX_RPC_REQUEST_BYTES", text = .doMaxRpcRequestBytes),
    (name = "DO_MAX_RPC_RESPONSE_BYTES", text = .doMaxRpcResponseBytes),
    (name = "DO_MAX_FETCH_BODY_BYTES", text = .doMaxFetchBodyBytes),
    (name = "DO_DISPATCH_TIMEOUT_MS", text = .doDispatchTimeoutMs),
    (name = "DO_MAX_IN_FLIGHT_DISPATCHES", text = .doMaxInFlightDispatches),
  ],
  globalOutbound = "outbound-gateway",
);

const doHostWorker :Workerd.Worker = (
  compatibilityDate = "2026-08-22",
  compatibilityFlags = ["nodejs_compat", "rpc", "enable_ctx_exports", "experimental"],
  modules = [
    (name = "durable-objects/router.js", esModule = embed "dist/durable-objects/router.js"),
    (name = "durable-objects/host.js", esModule = embed "dist/durable-objects/host.js"),
    (name = "loader/host.js", esModule = embed "dist/loader/host.js"),
    (name = "loader/shared.js", esModule = embed "dist/loader/shared.js"),
    (name = "services/transport.js", esModule = embed "dist/services/transport.js"),
    (name = "assets/router.js", esModule = embed "dist/assets/router.js"),
    (name = "workflows/host.js", esModule = embed "dist/workflows/host.js"),
    (name = "workflows/controller.js", esModule = embed "dist/workflows/controller.js"),
    (name = "loader/modules.js", esModule = embed "dist/loader/modules.js"),
    (name = "loader/snapshot.js", esModule = embed "dist/loader/snapshot.js"),
    (name = "kv/transport.js", esModule = embed "dist/kv/transport.js"),
    (name = "loader/wrapper-runtime-source", text = embed "dist/loader/wrappers/runtime.js"),
    (name = "loader/do-wrapper-source", text = embed "dist/loader/wrappers/durable-object.js"),
    (name = "loader/workflow-wrapper-source", text = embed "dist/loader/wrappers/workflow.js"),
    (name = "loader/workflow-runner-source", text = embed "dist/workflows/runner.js"),
    (name = "loader/workflow-json-source", text = embed "dist/workflows/json.js"),
    (name = "loader/workflow-facade-source", text = embed "dist/workflows/facade.js"),
    (name = "loader/r2-facade-source", text = embed "dist/r2/facade.js"),
    (name = "loader/d1-facade-source", text = embed "dist/d1/facade.js"),
    (name = "loader/do-facade-source", text = embed "dist/durable-objects/facade.js"),
    (name = "loader/do-id-codec-source", text = embed "dist/durable-objects/id-codec.js"),
    (name = "loader/do-alarm-shim-source", text = embed "dist/durable-objects/alarm-shim.js"),
    (name = "loader/queue-facade-source", text = embed "dist/queues/facade.js"),
    (name = "loader/assets-facade-source", text = embed "dist/assets/facade.js"),
    (name = "loader/service-facade-source", text = embed "dist/services/facade.js"),
    (name = "loader/service-scope-source", text = embed "dist/services/scope.js"),
    (name = "loader/wrappers/generator.js", esModule = embed "dist/loader/wrappers/generator.js"),
    (name = "workflows/binding.js", esModule = embed "dist/workflows/binding.js"),
    (name = "loader/bindings.js", esModule = embed "dist/loader/bindings.js"),
    (name = "r2/transport.js", esModule = embed "dist/r2/transport.js"),
    (name = "d1/transport.js", esModule = embed "dist/d1/transport.js"),
  ],
  bindings = [
    (name = "LOADER", workerLoader = (id = "open-compute")),
    (name = "RUNTIME_SOURCE", service = "runtime-source"),
    (name = "BINDING_BACKEND", service = "binding-backend"),
    (name = "BINDING_BACKEND_TOKEN", text = .bindingToken),
    (name = "INTERNAL_TOKEN", text = .internalToken),
    (name = "DO_ROUTER", service = "do-router"),
    (name = "DO_HOST", durableObjectNamespace = "DoHost"),
    (name = "DO_MAX_OBJECT_NAME_BYTES", text = .doMaxObjectNameBytes),
    (name = "DO_MAX_RPC_REQUEST_BYTES", text = .doMaxRpcRequestBytes),
    (name = "DO_MAX_RPC_RESPONSE_BYTES", text = .doMaxRpcResponseBytes),
    (name = "DO_MAX_FETCH_BODY_BYTES", text = .doMaxFetchBodyBytes),
    (name = "DO_DISPATCH_TIMEOUT_MS", text = .doDispatchTimeoutMs),
    (name = "DO_MAX_IN_FLIGHT_DISPATCHES", text = .doMaxInFlightDispatches),
    (name = "DO_DISK_STOP_WRITES_PERCENT", text = .doDiskStopWritesPercent),
  ],
  durableObjectNamespaces = [
    (
      className = "DoHost",
      uniqueKey = "open-compute-do-host-v1",
      enableSql = true,
    ),
  ],
  durableObjectStorage = (localDisk = "do-storage"),
  globalOutbound = "outbound-gateway",
);

const outboundGatewayWorker :Workerd.Worker = (
  compatibilityDate = "2026-08-22",
  compatibilityFlags = ["nodejs_compat", "rpc", "enable_ctx_exports", "experimental"],
  modules = [
    (name = "gateway/outbound.js", esModule = embed "dist/gateway/outbound.js"),
  ],
  globalOutbound = "internet",
);
