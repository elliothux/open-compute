using Workerd = import "/workerd/workerd.capnp";
const config :Workerd.Config = (
  services = [
    (name = "probe", worker = (
      compatibilityDate = "2026-08-30",
      modules = [
        (name = "probe.js", esModule = embed "output-gate.js"),
        (name = "workflows/facade.js", esModule = embed "../../../../packages/runtime/dist/workflows/facade.js"),
        (name = "workflows/codec.js", esModule = embed "../../../../packages/runtime/dist/workflows/codec.js"),
        (name = "serialization/codec.js", esModule = embed "../../../../packages/runtime/dist/serialization/codec.js"),
        (name = "serialization/encode.js", esModule = embed "../../../../packages/runtime/dist/serialization/encode.js"),
        (name = "serialization/decode.js", esModule = embed "../../../../packages/runtime/dist/serialization/decode.js"),
        (name = "serialization/format.js", esModule = embed "../../../../packages/runtime/dist/serialization/format.js"),
        (name = "durable-objects/output-gate.js", esModule = embed "../../../../packages/runtime/dist/durable-objects/output-gate.js"),
        (name = "durable-objects/alarm-shim.js", esModule = embed "../../../../packages/runtime/dist/durable-objects/alarm-shim.js"),
      ],
      bindings = [
        (name = "PROBES", durableObjectNamespace = "Caller"),
        (name = "STORE", durableObjectNamespace = "Store"),
        (name = "BACKEND", service = (name = "probe", entrypoint = "Backend")),
      ],
      durableObjectNamespaces = [
        (className = "Caller", uniqueKey = "workflow-output-gate-caller", enableSql = true),
        (className = "Store", uniqueKey = "workflow-output-gate-store", enableSql = true),
      ],
      durableObjectStorage = (localDisk = "storage"),
    )),
    (name = "storage", disk = (path = ".", writable = true)),
  ],
);
