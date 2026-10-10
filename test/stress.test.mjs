import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { registerHooks } from "node:module";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const exec = promisify(execFile);
const root = fileURLToPath(new URL("../", import.meta.url));
const temporary = join(root, ".temp/stress-tests");

async function fixture() {
  await mkdir(temporary, { recursive: true });
  return mkdtemp(join(temporary, "run-"));
}

async function report(directory, mode = "peak", restarts = 0) {
  const { stdout } = await exec("python3", [
    join(root, "test/stress/report.py"),
    "--directory",
    directory,
    "--mode",
    mode,
    "--profile",
    "2c4g",
    "--stacks",
    "http",
    "kv",
    "--run-id",
    "fixture",
    "--timestamp",
    "2026-10-08T00:00:00Z",
    "--restarts",
    String(restarts),
  ]);
  return JSON.parse(stdout);
}

test("P0, peak and soak reject failed requests, excessive latency, and empty stacks", async () => {
  const directory = await fixture();
  for (const stack of ["http", "kv"]) {
    await writeFile(join(directory, `lat-${stack}.txt`), "10\n20\n30\n");
    await writeFile(join(directory, `err-${stack}.txt`), "");
  }
  await writeFile(join(directory, "soak-events.jsonl"), "");
  assert.equal((await report(directory)).verdict, "pass");
  assert.equal((await report(directory, "soak")).verdict, "pass");
  await writeFile(
    join(directory, "err-http.txt"),
    "GET / 500\nGET / 500\nGET / 500\n",
  );
  for (const mode of ["p0", "peak", "soak"]) {
    const failed = await report(directory, mode);
    assert.equal(failed.verdict, "fail");
    assert.equal(failed.stacks.http.error_rate, 1);
    assert.equal(failed.stacks.http.verdict, "fail");
  }
  await writeFile(join(directory, "err-http.txt"), "");
  await writeFile(join(directory, "lat-http.txt"), "10000\n");
  assert.equal((await report(directory, "soak")).verdict, "fail");
  await writeFile(join(directory, "lat-http.txt"), "");
  const empty = await report(directory, "soak");
  assert.equal(empty.verdict, "fail");
  assert.ok(
    empty.stacks.http.anomalies.some((item) => item.type === "no_samples"),
  );
  await assert.rejects(report(directory, "soak", 1), /restart count/);
  await rm(directory, { recursive: true });
});

test("stress restart selects the Compose service and propagates failed or ineffective restarts", async () => {
  const directory = await fixture();
  const trace = join(directory, "trace");
  const started = join(directory, "started");
  await writeFile(
    join(directory, "docker"),
    `#!/bin/sh
printf '%s\n' "$*" >> "$TRACE"
case "$*" in
  *" ps -q ocd") printf '%s\n' fixture-container ;;
  *" restart ocd")
    [ "\${FAIL_RESTART:-0}" != 1 ] || exit 12
    [ "\${NO_RESTART:-0}" = 1 ] || touch "$STARTED"
    ;;
  "inspect --format {{.State.StartedAt}} fixture-container")
    if [ -f "$STARTED" ]; then echo after; else echo before; fi ;;
  *) exit 13 ;;
esac
`,
    { mode: 0o755 },
  );
  for (const command of ["pgrep", "pkill"]) {
    await writeFile(
      join(directory, command),
      `#!/bin/sh\necho unexpected-host-process-access >> "$TRACE"\nexit 99\n`,
      { mode: 0o755 },
    );
  }
  const env = {
    ...process.env,
    PATH: `${directory}:${process.env.PATH}`,
    TRACE: trace,
    STARTED: started,
    OPEN_COMPUTE_ROOT: root,
    STRESS_ACCOUNT_ID: "fixture-account",
  };
  const script = `. '${join(root, "test/stress/common.sh")}'; restart_stress_container`;
  await exec("bash", ["-c", script], { env });
  const calls = await readFile(trace, "utf8");
  assert.match(calls, /compose .* ps -q ocd/);
  assert.match(calls, /inspect .*fixture-container/);
  assert.match(calls, /compose .* restart ocd/);
  assert.doesNotMatch(calls, /open-compute-ocd|unexpected-host-process-access/);
  await assert.rejects(
    exec("bash", ["-c", script], { env: { ...env, FAIL_RESTART: "1" } }),
    { code: 1 },
  );
  await rm(started);
  await assert.rejects(
    exec("bash", ["-c", script], { env: { ...env, NO_RESTART: "1" } }),
    /did not restart/,
  );
  await rm(directory, { recursive: true });
});

test("readiness helpers preserve caller variables and soak executes every stack", async () => {
  const directory = await fixture();
  const source = await readFile(
    join(root, "test/stress/stress-p1-soak.sh"),
    "utf8",
  );
  const soak = source.match(/^run_stack_soak\(\) \{\n[\s\S]*?^\}/m)?.[0];
  assert.ok(soak);
  const { stdout } = await exec(
    "bash",
    [
      "-c",
      `
set -eu
. '${join(root, "test/stress/common.sh")}'
curl() { printf '%s' "$CURL_CODE"; }
sleep() { :; }
CURL_CODE=200
stack=caller
attempts=12
stable=7
base=caller-base
ready=caller-ready
settle_sec=23
wait_for_ready "$base_url" 2
wait_for_worker_ready 2
recover_before_sample 0
[ "$stack/$attempts/$stable/$base/$ready/$settle_sec" = 'caller/12/7/caller-base/caller-ready/23' ]
CURL_CODE=503
if wait_for_ready "$base_url" 2; then exit 8; fi
if wait_for_worker_ready 2; then exit 9; fi
[ "$stack/$attempts/$stable/$base/$ready/$settle_sec" = 'caller/12/7/caller-base/caller-ready/23' ]
CURL_CODE=200
SOAK_RECOVER_SETTLE_SEC=0
SOAK_HTTP_CONCURRENCY=1 SOAK_KV_CONCURRENCY=1 SOAK_D1_CONCURRENCY=1
SOAK_R2_RATE=1 SOAK_QUEUE_RATE=1 SOAK_DO_RATE=1 SOAK_WORKFLOW_RATE=1
SOAK_FETCH_CONCURRENCY=1 SOAK_MEGA_CONCURRENCY=1
run_id=fixture
run_duration_concurrent() { printf '%s %s\\n' "$1" "$3"; }
run_rate_load() { printf '%s %s\\n' "$1" "$3"; }
${soak}
for name in http kv d1 r2 queue do workflow fetch scenario_mega; do
  run_stack_soak "$name" 300
  [ "$stack" = "$name" ]
done
`,
    ],
    {
      env: {
        ...process.env,
        OPEN_COMPUTE_ROOT: root,
        STRESS_ACCOUNT_ID: "fixture-account",
        STRESS_RUN_DIR: directory,
      },
    },
  );
  assert.equal(
    stdout,
    [
      "http",
      "kv",
      "d1",
      "r2",
      "queue",
      "do",
      "workflow",
      "fetch",
      "scenario_mega",
    ]
      .map((name) => `${name} 300\n`)
      .join(""),
  );
  await rm(directory, { recursive: true });
});

test("release qualification selects five stages and retains soak recovery evidence", async () => {
  const directory = await fixture();
  try {
    const source = await readFile(
      join(root, "test/stress/qualify-release.sh"),
      "utf8",
    );
    const collector = source.match(
      /python3 - "\$STRESS_RUN_ROOT" "\$package_report" <<'PY'\n([\s\S]*?)\nPY/,
    )?.[1];
    assert.ok(collector);
    const packagePath = join(directory, "package.json");
    await writeFile(
      packagePath,
      JSON.stringify({
        version: "1.2.3",
        revision: "fixture",
        sha256: "fixture",
        workerdLockSha256: "fixture",
      }),
    );
    const profiles = [
      "smoke",
      "p0-2c4g",
      "scenario",
      "p1-2c4g-peak",
      "p1-2c4g-soak",
    ];
    const events = [{ event: "restart_reconcile_ok" }];
    for (const [index, profile] of [
      ...profiles,
      "reconcile",
      "reconcile",
      "reconcile",
    ].entries()) {
      await mkdir(join(directory, String(index)));
      await writeFile(
        join(directory, String(index), "result.json"),
        JSON.stringify({
          profile,
          verdict: "pass",
          ...(profile.endsWith("soak") ? { soak: { events } } : {}),
        }),
      );
    }
    const collect = () =>
      exec("python3", ["-c", collector, directory, packagePath]);
    await collect();
    const qualification = JSON.parse(
      await readFile(join(directory, "qualification.json"), "utf8"),
    );
    assert.deepEqual(
      qualification.runs.map((run) => run.profile),
      profiles,
    );
    assert.deepEqual(qualification.runs.at(-1).soak.events, events);
    const first = join(directory, "0/result.json");
    await writeFile(
      first,
      JSON.stringify({ profile: "smoke", verdict: "fail" }),
    );
    await assert.rejects(collect());
    await writeFile(
      first,
      JSON.stringify({ profile: "scenario", verdict: "pass" }),
    );
    await assert.rejects(collect());
    await rm(first);
    await assert.rejects(collect());
  } finally {
    await rm(directory, { recursive: true });
  }
});

test("final qualification exits unsuccessfully and preserves failed evidence", async () => {
  const directory = await fixture();
  const result = join(directory, "result.json");
  const script = `. '${join(root, "test/stress/lib/anomaly-check.sh")}'; finalize_verdict '${result}'`;
  const env = {
    ...process.env,
    STRESS_RUN_DIR: directory,
    STRESS_BASE_URL: "http://127.0.0.1:8788",
  };
  await writeFile(
    result,
    JSON.stringify({ verdict: "fail", stacks: { http: { errors: 1 } } }),
  );
  await assert.rejects(exec("bash", ["-c", script], { env }), { code: 1 });
  assert.equal(
    JSON.parse(await readFile(join(directory, "failed/result.json"), "utf8"))
      .verdict,
    "fail",
  );
  assert.equal(
    JSON.parse(await readFile(join(directory, "anomalies.json"), "utf8"))
      .verdict,
    "fail",
  );
  await rm(directory, { recursive: true });
});

test("queue peak verifies a sent label without reading the sender's local counter", async () => {
  const directory = await fixture();
  const source = await readFile(join(root, "test/stress/stress-p1.sh"), "utf8");
  const queue = source.match(/^stack_queue_peak\(\) \{\n[\s\S]*?^\}/m)?.[0];
  assert.ok(queue);
  const { stdout } = await exec(
    "bash",
    [
      "-c",
      `
set -eu
run_id=fixture
P1_QUEUE_RATE=1
init_stack_files() { : > "$STRESS_RUN_DIR/lat.txt"; }
stack_lat_file() { echo "$STRESS_RUN_DIR/lat.txt"; }
stack_err_file() { echo "$STRESS_RUN_DIR/err.txt"; }
p1_duration_sec() { echo 1; }
p1_rate() { echo 1; }
run_rate_until() { local seq=0; while [ "$seq" -lt "$count" ]; do echo 10 >> "$7"; seq=$((seq+1)); done; }
wait_queue_label() { echo "$1"; }
sample_stack_response() { :; }
stack_stats() { :; }
record_anomaly() { exit 9; }
${queue}
count=45
stack_queue_peak
count=12
stack_queue_peak
`,
    ],
    { env: { ...process.env, STRESS_RUN_DIR: directory } },
  );
  assert.equal(stdout, "p1-q-fixture-25\np1-q-fixture-1\n");
  await rm(directory, { recursive: true });
});

test("peak rate sender runs each batch concurrently and waits for its samples", async () => {
  const directory = await fixture();
  const source = await readFile(join(root, "test/stress/stress-p1.sh"), "utf8");
  const sender = source.match(/^run_rate_until\(\) \{\n[\s\S]*?^\}/m)?.[0];
  assert.ok(sender);
  await exec(
    "bash",
    [
      "-c",
      `
set -eu
run_id=fixture
date() { if [ -f "$STRESS_RUN_DIR/complete" ]; then echo 101; else echo 100; fi; }
stress_request() {
  local marker
  marker=$(printf '%s' "$3" | jq -r .label)
  touch "$STRESS_RUN_DIR/started-$marker"
  while [ "$(find "$STRESS_RUN_DIR" -name 'started-*' | wc -l)" -lt 4 ]; do sleep .01; done
  printf '%s\n' "$3" >> "$STRESS_RUN_DIR/bodies.txt"
  echo 10 >> "$4"
  touch "$STRESS_RUN_DIR/complete"
}
${sender}
sleep 10 >/dev/null 2>&1 &
unrelated=$!
trap 'kill "$unrelated" 2>/dev/null || true' EXIT
run_rate_until fixture 4 1 /queue POST '{"label":"__RUN__-__SEQ__"}' "$STRESS_RUN_DIR/lat.txt" "$STRESS_RUN_DIR/err.txt"
`,
    ],
    { env: { ...process.env, STRESS_RUN_DIR: directory }, timeout: 5000 },
  );
  const bodies = (await readFile(join(directory, "bodies.txt"), "utf8"))
    .trim()
    .split("\n")
    .map((body) => JSON.parse(body).label)
    .sort();
  assert.deepEqual(bodies, [
    "fixture-1",
    "fixture-2",
    "fixture-3",
    "fixture-4",
  ]);
  assert.equal(
    (await readFile(join(directory, "lat.txt"), "utf8")).trim().split("\n")
      .length,
    4,
  );
  await assert.rejects(
    exec(
      "bash",
      [
        "-c",
        `
set -eu
run_id=fixture
date() { echo 100; }
stress_request() { return 7; }
${sender}
run_rate_until fixture 4 1 /queue POST '' "$STRESS_RUN_DIR/lat.txt" "$STRESS_RUN_DIR/err.txt"
`,
      ],
      { env: { ...process.env, STRESS_RUN_DIR: directory }, timeout: 5000 },
    ),
    { code: 1 },
  );
  await rm(directory, { recursive: true });
});

test("request latency uses curl transfer time and failed transfers remain errors", async () => {
  const directory = await fixture();
  await writeFile(
    join(directory, "curl"),
    '#!/bin/sh\nprintf "200 0.432100\\n"\nexit "${CURL_EXIT:-0}"\n',
    { mode: 0o755 },
  );
  const env = {
    ...process.env,
    PATH: `${directory}:${process.env.PATH}`,
    OPEN_COMPUTE_ROOT: root,
    STRESS_ACCOUNT_ID: "fixture-account",
    STRESS_RUN_DIR: directory,
  };
  const script = `set -eu; . '${join(root, "test/stress/common.sh")}'; now_ms() { exit 9; }; stress_request /queue POST '{}' "$STRESS_RUN_DIR/lat.txt" "$STRESS_RUN_DIR/err.txt"`;
  await exec("bash", ["-c", script], { env });
  await exec("bash", ["-c", script], { env: { ...env, CURL_EXIT: "56" } });
  assert.equal(
    await readFile(join(directory, "lat.txt"), "utf8"),
    "432\n432\n",
  );
  assert.equal(
    await readFile(join(directory, "err.txt"), "utf8"),
    "POST /queue 000\n",
  );
  await rm(directory, { recursive: true });
});

test("queue consumer persists evidence before ACK without a shared DO counter", async () => {
  const workers =
    "data:text/javascript," +
    encodeURIComponent(
      "export class DurableObject {}; export class WorkerEntrypoint {}; export class WorkflowEntrypoint {};",
    );
  const source = new URL("../examples/stress-demo/src/", import.meta.url);
  const hooks = registerHooks({
    resolve(specifier, context, nextResolve) {
      if (specifier === "cloudflare:workers")
        return { url: workers, shortCircuit: true };
      if (
        context.parentURL?.startsWith(source.href) &&
        specifier.startsWith(".")
      )
        return nextResolve(
          new URL(`${specifier}.ts`, context.parentURL).href,
          context,
        );
      return nextResolve(specifier, context);
    },
  });
  let worker;
  try {
    ({ default: worker } = await import(new URL("index.ts", source).href));
  } finally {
    hooks.deregister();
  }
  const trace = [];
  const env = {
    KV: {
      async put(key, value) {
        trace.push([key, value]);
      },
      async get() {
        return "processed";
      },
    },
    get INVENTORY() {
      throw new Error("queue processing must not require a shared DO");
    },
  };
  const messages = [
    {
      body: { label: "first" },
      ack() {
        trace.push(["ack", "first"]);
      },
    },
    {
      body: { label: "second", orderId: "order", phase: "committed" },
      ack() {
        trace.push(["ack", "second"]);
      },
    },
  ];
  await worker.queue({ messages }, env);
  assert.deepEqual(
    trace.map(([key]) => key),
    [
      "queue-processed:first",
      "ack",
      "queue-processed:second",
      "queue-order:order:second",
      "ack",
    ],
  );
  assert.equal(trace[2][1], "committed");
  const verification = await worker.fetch(
    new Request("http://fixture/stack/queue/dequeue-verify?label=first"),
    env,
  );
  assert.equal((await verification.json()).processed, true);
  trace.length = 0;
  env.KV.put = async () => {
    throw new Error("KV write failed");
  };
  await assert.rejects(worker.queue({ messages }, env), /KV write failed/);
  assert.deepEqual(trace, []);
});

test("stress bootstrap creates a 100-message consumer and rejects mismatched settings", async () => {
  const directory = await fixture();
  const source = await readFile(
    join(root, "test/stress/deploy-stress-demo.sh"),
    "utf8",
  );
  const start = source.indexOf("consumers=$(cf queues consumers list");
  const end = source.indexOf(
    'sh "${root}/test/stress/generate-data.sh"',
    start,
  );
  assert.ok(start >= 0 && end > start);
  const configuration = source.slice(start, end);
  const inventory = join(directory, "consumers.json");
  const consumer = {
    script_name: "stress-demo",
    type: "worker",
    settings: { batch_size: 100 },
  };
  const script = `set -eu
queue_id=fixture
cf() {
  if [ "$1 $2 $3" = "queues consumers list" ]; then cat "$INVENTORY"; return; fi
  [ "$1 $2 $3" = "queues consumers create" ] || exit 19
  case " $* " in *" --settings-batch-size 100 "*) ;; *) exit 20;; esac
  printf '%s' "$EXPECTED" > "$INVENTORY"
}
${configuration}`;
  const env = {
    ...process.env,
    INVENTORY: inventory,
    EXPECTED: JSON.stringify([consumer]),
  };
  for (const initial of [[], [consumer]]) {
    await writeFile(inventory, JSON.stringify(initial));
    await exec("bash", ["-c", script], { env });
    assert.deepEqual(JSON.parse(await readFile(inventory, "utf8")), [consumer]);
  }
  await writeFile(
    inventory,
    JSON.stringify([{ ...consumer, settings: { batch_size: 10 } }]),
  );
  await assert.rejects(exec("bash", ["-c", script], { env }), /batch size 100/);
  await rm(directory, { recursive: true });
});
