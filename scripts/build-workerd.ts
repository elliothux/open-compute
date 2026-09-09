import { verifyBundledPyodide } from "./bundled-pyodide.ts";
import { bundledWorkerdArchive } from "./bundled-workerd.ts";
import { loadPin, loadPyodidePin, repository } from "./workerd-archive.ts";

// Prepare the official release targets from their checked-in binaries; never download.
// The darwin-x64 pin remains available for explicit manual Intel builds.
for (const target of ["darwin-arm64", "linux-arm64", "linux-x64"]) {
  const pin = await loadPin(target);
  await bundledWorkerdArchive(repository, pin);
  console.log(`Verified bundled workerd: ${target}`);
}

const pyodide = await loadPyodidePin();
await verifyBundledPyodide(repository, pyodide);
console.log(`Verified bundled Pyodide: ${pyodide.version}`);
