using Workerd = import "/workerd/workerd.capnp";

const config :Workerd.Config = (
  services = [
    (name = "service-bindings-hard", worker = (
      compatibilityDate = "2026-08-26",
      compatibilityFlags = ["service_binding_extra_handlers"],
      modules = [(name = "probe.js", esModule = embed "probe.js")],
      bindings = [
        (name = "BACKEND", service = (name = "service-bindings-hard", entrypoint = "Backend")),
        (name = "TRANSPORT", service = (name = "service-bindings-hard", entrypoint = "Transport")),
      ],
    )),
  ],
);
