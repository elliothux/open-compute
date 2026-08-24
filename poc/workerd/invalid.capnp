using Workerd = import "/workerd/workerd.capnp";

const config :Workerd.Config = (
  services = [
    (name = "broken", worker = (
      compatibilityDate = "not-a-date",
      modules = [
        (name = "missing-embed.js", esModule = embed "this-file-does-not-exist.js"),
      ],
    )),
  ],
  sockets = [
    (name = "http", address = "127.0.0.1:0", http = (), service = "broken"),
  ],
);
