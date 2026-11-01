# Python runtime and FFI fixture

The maintained cf configuration owns bindings and Python flags. Build with
`bun run build:python test/applications/python-runtime --assets-directory public`
after installing PyWrangler yourself (`uv tool install workers-py`) and explicitly
preparing its Emscripten interpreter and dependencies. The bridge selects the
project `.venv/bin/pywrangler`, then PATH; missing tools fail with installation
guidance. It neither installs nor checks their version. Only Python build uses
PyWrangler; upload, deploy, development and resource management use cf.
TODO(P25): remove the bridge when cf has a qualified Python builder.

The reviewed raw cf upload and assets requests are in
`../../fixtures/python-runtime/`. The single `python-runtime` case verifies
maintained main/helper bytes and all 19 unmodified SDK sources before using the
real daemon, formal workerd pin, SQLite and SigV4 fixture. Loopback HTTP/TCP and
model services are owned by the Rust fixture. Static capture alone is not a
product qualification result; acceptance records belong to the completed Python
implementation report.

SDK, raw JavaScript FFI and a JavaScript peer compare shared KV/D1/R2, structured
values and byte data. Python-only assertions cover PyProxy callback destruction
and the current pinned standard library. `fcntl`, `termios`, `pty` and `tty`
import successfully without proving host POSIX operations; thread startup fails
and `multiprocessing` is checked only for import. Temporary files are absent
after fresh daemon or Version restore. waitUntil effects persist. HTTP and TCP
use the actual host network; half-open sockets are explicitly closed before
awaiting their closed promise.

`cache_cases.py` uses native `js.caches` with SDK Request/Response conversion or
raw FFI; the SDK has no separate Cache wrapper. Default and named namespaces
remain isolated, and entries are Worker-scoped mutable state shared across
Versions. A second-version default-cache write survives rollback and restart;
named entries and the JavaScript Worker's state remain independent. Automatic
Workers Cache has a separate version policy. Conditional/range responses,
method options, invalid puts and delete results are checked.

`public/` assets are selected by the explicit builder option, while cf owns
`bindings.assets()` and Worker routing. SDK/raw FFI/JavaScript Fetcher requests
compare GET, HEAD, 304, missing files and invalid methods, including exact asset
bytes after immutable-version promotion, fresh restart and rollback. The Images
helper compares decoded raster outputs, transformations, draw, response headers
and streaming, and rejects malformed bytes/options. Asset Range behavior and
Cache API Range behavior are separate contracts.

AI Markdown Conversion, Vectorize, Artifacts and AI Search use the same existing
backends as JavaScript. Each helper validates the declared product subset and
persisted effects across restart and rollback. No Python facade is introduced
for extensions without an SDK wrapper: both env paths use explicit FFI. Issued
Artifacts token plaintext is checked inside the Worker and removed from results.
AI Search uses bounded local embedding/chat services and a fixed tokenizer;
this does not qualify Cloudflare-hosted models. The namespace in binding
instance-info is checked against persisted resource identity. Unsupported AI
inference and new disposable Git/content APIs remain outside this fixture.

Secrets, encrypted prepared identities, failure responses and cleanup are checked
throughout the same lifecycle. The case finishes by proving leases, processes
and owned listeners are released. Evidence and known limitations are recorded
in the P21 implementation record and the compatibility/testing references.
