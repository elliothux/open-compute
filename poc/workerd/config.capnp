using Workerd = import "/workerd/workerd.capnp";

const config :Workerd.Config = (
  structuredLogging = true,

  services = [
    (name = "ingress", worker = .ingressWorker),
    (name = "echo", worker = .echoWorker),
    (name = "loader-host", worker = .loaderHostWorker),
    (name = "binding-host", worker = .bindingHostWorker),
    (name = "do-supervisor", worker = .doSupervisorWorker),
    (name = "g0-do-disk", disk = (writable = true, allowDotfiles = true)),
    (name = "g0-fixtures", disk = (writable = false)),
  ],

  sockets = [
    (name = "http", address = "127.0.0.1:0", http = (), service = "ingress"),
  ],
);

const ingressWorker :Workerd.Worker = (
  compatibilityDate = "2026-08-22",
  compatibilityFlags = ["nodejs_compat", "rpc", "enable_ctx_exports", "experimental"],
  modules = [
    (name = "ingress.js", esModule = embed "ingress.js"),
    (name = "log.js", esModule = embed "log.js"),
    (name = "errors.js", esModule = embed "errors.js"),
    (name = "registry.js", esModule = embed "registry.js"),
  ],
  bindings = [
    (name = "ECHO", service = "echo"),
    (name = "ECHO_NAMED", service = (name = "echo", entrypoint = "named")),
    (name = "LOADER_HOST", service = "loader-host"),
    (name = "BINDING_HOST", service = "binding-host"),
    (name = "DO_SUPERVISOR", service = "do-supervisor"),
  ],
  globalOutbound = "echo",
);

const echoWorker :Workerd.Worker = (
  compatibilityDate = "2026-08-22",
  compatibilityFlags = ["nodejs_compat", "rpc", "enable_ctx_exports", "experimental"],
  modules = [
    (name = "echo.js", esModule = embed "echo.js"),
    (name = "log.js", esModule = embed "log.js"),
  ],
  globalOutbound = "echo",
);

const loaderHostWorker :Workerd.Worker = (
  compatibilityDate = "2026-08-22",
  compatibilityFlags = ["nodejs_compat", "rpc", "enable_ctx_exports", "experimental"],
  modules = [
    (name = "loader-host.js", esModule = embed "loader-host.js"),
    (name = "log.js", esModule = embed "log.js"),
    (name = "errors.js", esModule = embed "errors.js"),
    (name = "registry.js", esModule = embed "registry.js"),
    (name = "code.js", esModule = embed "code.js"),
  ],
  bindings = [
    (name = "LOADER", workerLoader = (id = "g0")),
    (name = "FIXTURES", service = "g0-fixtures"),
    (name = "BINDING_BACKEND", service = (name = "binding-host", entrypoint = "BindingBackend")),
  ],
  globalOutbound = "echo",
);

const bindingHostWorker :Workerd.Worker = (
  compatibilityDate = "2026-08-22",
  compatibilityFlags = ["nodejs_compat", "rpc", "enable_ctx_exports", "experimental"],
  modules = [
    (name = "binding-host.js", esModule = embed "binding-host.js"),
    (name = "log.js", esModule = embed "log.js"),
    (name = "errors.js", esModule = embed "errors.js"),
  ],
  globalOutbound = "echo",
);

const doSupervisorWorker :Workerd.Worker = (
  compatibilityDate = "2026-08-22",
  compatibilityFlags = ["nodejs_compat", "rpc", "enable_ctx_exports", "experimental"],
  modules = [
    (name = "do-supervisor.js", esModule = embed "do-supervisor.js"),
    (name = "log.js", esModule = embed "log.js"),
    (name = "errors.js", esModule = embed "errors.js"),
    (name = "registry.js", esModule = embed "registry.js"),
    (name = "code.js", esModule = embed "code.js"),
  ],
  bindings = [
    (name = "LOADER", workerLoader = (id = "g0")),
    (name = "FIXTURES", service = "g0-fixtures"),
    (name = "DoSupervisor", durableObjectNamespace = "DoSupervisor"),
  ],
  durableObjectNamespaces = [
    (
      className = "DoSupervisor",
      uniqueKey = "g0-do-supervisor-unique-key-v1",
      enableSql = true,
      preventEviction = true,
    ),
  ],
  durableObjectStorage = (localDisk = "g0-do-disk"),
  globalOutbound = "echo",
);
