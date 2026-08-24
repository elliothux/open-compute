"use strict";

const fs = require("node:fs");
const { acquireWorkerd } = require("../harness/runtime");
const { WorkerdProcess } = require("../harness/process-supervisor");

(async () => {
  const pidPath = process.argv[2];
  const runDir = process.argv[3];
  if (!pidPath || !runDir) {
    throw new Error("usage: orphan-helper.js <pid-path> <run-dir>");
  }
  const acquired = await acquireWorkerd();
  const proc = new WorkerdProcess({
    binPath: acquired.binPath,
    lock: acquired.lock,
    runDir,
  });
  await proc.start();
  fs.writeFileSync(pidPath, `${proc.pid}\n`);
  process.exit(0);
})().catch((err) => {
  process.stderr.write(String(err && err.stack ? err.stack : err) + "\n");
  process.exit(1);
});
