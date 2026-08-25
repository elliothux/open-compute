using Workerd = import "/workerd/workerd.capnp";

const config :Workerd.Config = (
  structuredLogging = true,

  services = [
    (name = "ingress", worker = .ingressWorker),
    (name = "loader-host", worker = .loaderHostWorker),
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
    (name = "INTERNAL_TOKEN", text = "__OPEN_COMPUTE_INTERNAL_TOKEN__"),
    (name = "LOADER_HOST", service = "loader-host"),
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
    (name = "r2-wrapper-generator.js", esModule = embed "system-workers/r2-wrapper-generator.js"),
    (name = "r2-transport.js", esModule = embed "system-workers/r2-transport.js"),
    (name = "d1-transport.js", esModule = embed "system-workers/d1-transport.js"),
  ],
  bindings = [
    (name = "LOADER", workerLoader = (id = "open-compute")),
    (name = "RUNTIME_SOURCE", service = "runtime-source"),
    (name = "BINDING_BACKEND", service = "binding-backend"),
    (name = "BINDING_BACKEND_TOKEN", text = "__OPEN_COMPUTE_BINDING_TOKEN__"),
  ],
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
