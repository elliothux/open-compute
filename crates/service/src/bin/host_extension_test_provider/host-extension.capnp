# Host extension session contract between workerd and an operator-owned native Provider.
# Copied from third_party/workerd (elliothux fork) revision 40937077470ed7edec082329d3a10e4195b402cb,
# path src/workerd/io/host-extension.capnp. Sync this copy whenever a coordinated workerd pin
# update changes the upstream schema, then regenerate the committed Rust bindings in this
# directory (the compiler writes host_extension_capnp.rs to the working directory; commit it
# renamed to host_extension_capnp_fixture.rs so source policy classifies it as test source):
#   capnp compile -o capnpc-rust \
#     -I "$(brew --prefix capnp)/include" \
#     --src-prefix crates/service/src/bin/host_extension_test_provider \
#     crates/service/src/bin/host_extension_test_provider/host-extension.capnp

@0xa6cf773127c85b37;

using Cxx = import "/capnp/c++.capnp";
$Cxx.namespace("workerd::rpc");

# Direct, session-scoped data plane between workerd and an operator-owned native provider.
interface HostExtension {
  call @0 (method :UInt32, payload :Data) -> (payload :Data);
  openStream @1 (method :UInt32, payload :Data) -> (stream :HostExtensionStream);
}

interface HostExtensionStream {
  read @0 (maxBytes :UInt32) -> (payload :Data, eof :Bool);
  cancel @1 ();
}
