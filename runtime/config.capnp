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
    (name = "ingress.js", esModule = embed "system-workers/ingress.js"),
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
  compatibilityFlags = ["nodejs_compat", "rpc", "enable_ctx_exports", "experimental"],
  modules = [
    (name = "loader-host.js", esModule = embed "system-workers/loader-host.js"),
    (name = "r2-facade-source", text = embed "system-workers/r2-facade.js"),
    (name = "d1-facade-source", text = embed "system-workers/d1-facade.js"),
    (name = "do-facade-source", text = embed "system-workers/do-facade.js"),
    (name = "do-id-codec-source", text = embed "system-workers/do-id-codec.js"),
    (name = "do-alarm-shim-source", text = embed "system-workers/do-alarm-shim.js"),
    (name = "queue-facade-source", text = embed "system-workers/queue-facade.js"),
    (name = "loaded-isolate-wrapper-generator.js", esModule = embed "system-workers/loaded-isolate-wrapper-generator.js"),
    (name = "r2-transport.js", esModule = embed "system-workers/r2-transport.js"),
    (name = "d1-transport.js", esModule = embed "system-workers/d1-transport.js"),
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
    (name = "do-router.js", esModule = embed "system-workers/do-router.js"),
    (name = "do-host.js", esModule = embed "system-workers/do-host.js"),
    (name = "loader-host.js", esModule = embed "system-workers/loader-host.js"),
    (name = "r2-facade-source", text = embed "system-workers/r2-facade.js"),
    (name = "d1-facade-source", text = embed "system-workers/d1-facade.js"),
    (name = "do-facade-source", text = embed "system-workers/do-facade.js"),
    (name = "do-id-codec-source", text = embed "system-workers/do-id-codec.js"),
    (name = "do-alarm-shim-source", text = embed "system-workers/do-alarm-shim.js"),
    (name = "queue-facade-source", text = embed "system-workers/queue-facade.js"),
    (name = "loaded-isolate-wrapper-generator.js", esModule = embed "system-workers/loaded-isolate-wrapper-generator.js"),
    (name = "r2-transport.js", esModule = embed "system-workers/r2-transport.js"),
    (name = "d1-transport.js", esModule = embed "system-workers/d1-transport.js"),
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
    (name = "outbound-gateway.js", esModule = embed "system-workers/outbound-gateway.js"),
  ],
  globalOutbound = "internet",
);
