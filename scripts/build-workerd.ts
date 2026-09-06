import { bundledWorkerdArchive } from "./bundled-workerd.ts";
import { loadPin, repository } from "./workerd-archive.ts";

// Prepare all supported Cargo targets from their checked-in binaries; never download.
for (const target of ["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64"]) {
  const pin = await loadPin(target);
  await bundledWorkerdArchive(repository, pin);
  console.log(`Verified bundled workerd: ${target}`);
}
