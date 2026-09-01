using Workerd = import "/workerd/workerd.capnp";
const config :Workerd.Config = (
  services = [
    (name = "probe", worker = (
      compatibilityDate = "2026-08-30",
      compatibilityFlags = ["experimental", "unsafe_module"],
      modules = [
        (name = "probe.js", esModule = embed "hibernation-probe.js"),
      ],
      bindings = [
        (name = "ROOMS", durableObjectNamespace = "Room"),
      ],
      durableObjectNamespaces = [
        (className = "Room", uniqueKey = "do-hibernation-probe-room", enableSql = true),
      ],
      durableObjectStorage = (localDisk = "storage"),
    )),
    (name = "storage", disk = (path = ".", writable = true)),
  ],
);
