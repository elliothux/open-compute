using Workerd = import "/workerd/workerd.capnp";

const config :Workerd.Config = (
  structuredLogging = true,

  services = [
    (name = "ingress", worker = .ingressWorker),
    (name = "loader-host", worker = .loaderHostWorker),
    # The address is deliberately omitted from the compiled config. platformd
    # injects a generation-local loopback listener with --external-addr.
    (name = "runtime-source", external = (http = ())),
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
  ],
  bindings = [
    (name = "LOADER", workerLoader = (id = "open-compute")),
    (name = "RUNTIME_SOURCE", service = "runtime-source"),
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
