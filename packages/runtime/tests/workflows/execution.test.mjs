import assert from "node:assert/strict";
import test from "node:test";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const workerModule = moduleUrl(`export class RpcTarget {}
  export class WorkflowEntrypoint { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }`);
const { WorkflowEntrypoint } = await import(workerModule);
const format = moduleUrl(await compileRuntime("serialization/format.ts"));
const encode = moduleUrl(
  await compileRuntime("serialization/encode.ts", { "./format.js": format }),
);
const decode = moduleUrl(
  await compileRuntime("serialization/decode.ts", { "./format.js": format }),
);
const durableCodec = moduleUrl(
  await compileRuntime("serialization/codec.ts", {
    "./format.js": format,
    "./encode.js": encode,
    "./decode.js": decode,
  }),
);
const workflowCodec = moduleUrl(
  await compileRuntime("workflows/codec.ts", {
    "../serialization/codec.js": durableCodec,
  }),
);
// Keep the runner's data-URL import graph below Bun's package-name limit. The
// duration parser itself has focused tests; this fixture needs only the value
// used by its dynamic-delay case.
const workflowDuration = moduleUrl(`export function durationMs(value) {
  if (value === "2 seconds") return 2000;
  if (typeof value === "number" && Number.isSafeInteger(value) && value >= 0) return value;
  throw new Error("WORKFLOW_DURATION_INVALID");
}`);
const { encodeWorkflowBase64 } = await import(workflowCodec);
const controllerModule = moduleUrl(
  await compileRuntime("workflows/controller.ts", {
    "cloudflare:workers": workerModule,
  }),
);
const workflowErrors = moduleUrl(
  "export class NonRetryableError extends Error {}",
);
const { NonRetryableError } = await import(workflowErrors);
const runner = moduleUrl(
  await compileRuntime("workflows/runner.ts", {
    "cloudflare:workers": workerModule,
    "cloudflare:workflows": workflowErrors,
    "./codec.js": workflowCodec,
    "./duration.js": workflowDuration,
  }),
);
const { WorkflowRunController, finishWorkflowRun, closeWorkflowRun } =
  await import(controllerModule);
const { runWorkflow } = await import(runner);
const identity = {
  instanceId: "private-instance",
  instanceGeneration: 1,
  runToken: "private-run-token",
};
const event = {
  externalInstanceId: "order",
  definitionName: "flow",
  createdAtMs: 123,
  payloadBase64: encodeWorkflowBase64({ value: 1 }),
  rollback: false,
};
const declaration = {
  ordinal: 0,
  kind: "do",
  name: "compute",
  nameCount: 1,
  config: {},
  dependencies: [],
  batchFirstOrdinal: 0,
  batchSize: 1,
};
const config = {
  timeout: 1000,
  retries: { limit: 0, delay: 0, backoff: "constant" },
};

function controller(t, fetch) {
  const value = new WorkflowRunController(
    {
      BINDING_BACKEND: { fetch },
    },
    identity,
    10_000,
  );
  t.after(() => value[closeWorkflowRun]());
  return value;
}

test("compiled Workflow controller keeps grants private and fences duplicate commits", async (t) => {
  const calls = [];
  const value = controller(t, async (url, request) => {
    const body = JSON.parse(request.body);
    calls.push({ url, body, headers: request.headers });
    return Response.json(
      url.endsWith("/claim-batch")
        ? {
            steps: [
              {
                state: "run",
                stepToken: "a".repeat(64),
                attempt: 1,
                remainingMs: 10_000,
                config,
              },
            ],
          }
        : { state: "complete", outputBase64: encodeWorkflowBase64(42) },
    );
  });
  const claim = await value.claimBatch({ steps: [declaration] });
  assert.deepEqual(claim, {
    steps: [{ ordinal: 0, state: "run", attempt: 1, config }],
  });
  assert.deepEqual(Object.keys(value), []);
  assert.ok(!JSON.stringify(claim).includes("Token"));
  const result = value.result(0);
  assert.deepEqual(
    await value.success({
      ordinal: 0,
      outputBase64: encodeWorkflowBase64(42),
      stepToken: "forged",
      runToken: "forged",
    }),
    { state: "complete", outputBase64: encodeWorkflowBase64(42) },
  );
  assert.equal(calls[1].body.stepToken, "a".repeat(64));
  assert.equal(calls[1].body.runToken, identity.runToken);
  assert.equal(
    calls[1].headers["x-open-compute-startup-generation"],
    undefined,
  );
  assert.deepEqual(await result, {
    state: "complete",
    outputBase64: encodeWorkflowBase64(42),
  });
  assert.deepEqual(
    await value.success({ ordinal: 0, outputBase64: encodeWorkflowBase64(43) }),
    { errorCode: "WORKFLOW_STEP_STALE" },
  );
  assert.equal(calls.length, 2);
  assert.deepEqual(await value.drain(), { ok: true });
  const final = {
    outcome: "complete",
    outputBase64: encodeWorkflowBase64(42),
    finalOrdinal: 1,
  };
  assert.deepEqual(await value[finishWorkflowRun](final), {
    result: final,
    drainIncomplete: false,
  });
});

test("malformed Workflow verdicts permanently close admission for that activation", async (t) => {
  for (const reply of [
    null,
    { state: "complete", outputBase64: 42 },
    { state: "failed", code: 42 },
    { state: "unknown" },
  ]) {
    const value = controller(t, async () => Response.json(reply));
    await assert.rejects(value.result(0), /WORKFLOW_RUNTIME_UNAVAILABLE/);
    await assert.rejects(
      value[finishWorkflowRun]({
        outcome: "complete",
        outputBase64: encodeWorkflowBase64("caught"),
        finalOrdinal: 0,
      }),
      /WORKFLOW_RUNTIME_UNAVAILABLE/,
    );
  }
});

test("compiled Workflow runner persists one mixed Promise graph without executing replayed callbacks", async () => {
  const declarations = [];
  class Flow extends WorkflowEntrypoint {
    async run(value, step) {
      assert.equal(value.timestamp.getTime(), 123);
      assert.deepEqual(value.payload, { value: 1 });
      const never = () => {
        throw new Error("replayed callback executed");
      };
      return Promise.all([
        step.do("first", never),
        step.sleep("briefly", 1),
        step.waitForEvent("approval", { type: "approved" }),
      ]);
    }
  }
  const result = await runWorkflow(Flow, {}, {}, event, {
    async claimBatch(body) {
      declarations.push(...body.steps);
      return {
        steps: body.steps.map((step) => ({
          ordinal: step.ordinal,
          state: "complete",
        })),
      };
    },
    async result(ordinal) {
      if (ordinal === 2)
        return {
          state: "event",
          type: "approved",
          payloadBase64: encodeWorkflowBase64(new Map([["ok", true]])),
          timestampMs: 124,
        };
      return {
        state: "complete",
        ...(ordinal === 0 ? { outputBase64: encodeWorkflowBase64(1) } : {}),
      };
    },
    async drain() {
      return { ok: true };
    },
  });
  assert.equal(result.outcome, "complete");
  assert.equal(result.finalOrdinal, 3);
  assert.deepEqual(
    declarations.map((step) => [
      step.kind,
      step.ordinal,
      step.batchFirstOrdinal,
      step.batchSize,
    ]),
    [
      ["do", 0, 0, 3],
      ["sleep", 1, 0, 3],
      ["wait_event", 2, 0, 3],
    ],
  );
});

test("compiled Workflow runner rejects incomplete joins before durable admission", async () => {
  for (const mode of ["overlap", "unjoined"]) {
    let claims = 0;
    class Flow extends WorkflowEntrypoint {
      async run(_event, step) {
        const first = step.do("first", () => 1);
        if (mode === "overlap") {
          try {
            await step.sleep("overlap", 1);
          } catch {}
          await first;
        }
        return "must fail";
      }
    }
    const result = await runWorkflow(Flow, {}, {}, event, {
      async claimBatch() {
        claims++;
        return { steps: [] };
      },
      async drain() {
        return { ok: true };
      },
    });
    assert.deepEqual(result, {
      outcome: "errored",
      errorCode: "WORKFLOW_NON_DETERMINISTIC",
      finalOrdinal: mode === "overlap" ? 2 : 1,
    });
    assert.equal(claims, 0);
  }
});

test("catching a malformed Workflow event cannot turn a protocol failure into success", async () => {
  class Flow extends WorkflowEntrypoint {
    async run(_event, step) {
      try {
        await step.waitForEvent("approval", { type: "approved" });
      } catch {
        return "caught";
      }
    }
  }
  for (const reply of [
    { state: "complete", outputBase64: encodeWorkflowBase64(null) },
    {
      state: "event",
      type: "approved",
      payloadBase64: "not-base64",
      timestampMs: 1,
    },
  ]) {
    await assert.rejects(
      runWorkflow(Flow, {}, {}, event, {
        async claimBatch({ steps }) {
          return {
            steps: steps.map((item) => ({
              ordinal: item.ordinal,
              state: "complete",
            })),
          };
        },
        async result() {
          return reply;
        },
        async drain() {
          return { ok: true };
        },
      }),
      /WORKFLOW_RUNTIME_UNAVAILABLE/,
    );
  }
});

test("waitForEvent preserves durable suspension and timeout verdicts", async () => {
  class Flow extends WorkflowEntrypoint {
    async run(_event, step) {
      return step.waitForEvent("approval", { type: "approved" });
    }
  }
  for (const [reply, expected] of [
    [{ state: "suspended" }, { outcome: "suspended", finalOrdinal: 1 }],
    [
      { state: "failed", code: "WORKFLOW_EVENT_TIMEOUT" },
      {
        outcome: "errored",
        errorCode: "WORKFLOW_EVENT_TIMEOUT",
        finalOrdinal: 1,
      },
    ],
  ]) {
    const result = await runWorkflow(Flow, {}, {}, event, {
      async claimBatch({ steps }) {
        return {
          steps: steps.map((item) => ({
            ordinal: item.ordinal,
            state: reply.state,
          })),
        };
      },
      async result() {
        return reply;
      },
      async drain() {
        return { ok: true };
      },
    });
    assert.deepEqual(result, expected);
  }
});

test("suspension crosses an exception-conversion boundary without exposing a stack", async () => {
  let converted;
  class Flow extends WorkflowEntrypoint {
    async run(_event, step) {
      try {
        await step.waitForEvent("approval", { type: "approved" });
      } catch (error) {
        converted = String(error);
        assert.equal(error.stack, converted);
        throw new Error(converted);
      }
    }
  }
  assert.deepEqual(
    await runWorkflow(Flow, {}, {}, event, {
      async claimBatch() {
        return { state: "suspended" };
      },
      async drain() {
        return { ok: true };
      },
    }),
    { outcome: "suspended", finalOrdinal: 1 },
  );
  assert.equal(converted, "Error: WORKFLOW_RUN_STALE");

  class Forged extends WorkflowEntrypoint {
    async run() {
      throw new Error(converted);
    }
  }
  assert.deepEqual(await runWorkflow(Forged, {}, {}, event, {}), {
    outcome: "errored",
    errorCode: "WORKFLOW_EXECUTION_FAILED",
    finalOrdinal: 0,
  });
});

test("step.do resolves a dynamic retry delay with the same tenant Error", async () => {
  let failureBody;
  let finishResult;
  const resultReady = new Promise((resolve) => {
    finishResult = resolve;
  });
  class Flow extends WorkflowEntrypoint {
    async run(_event, step) {
      return step.do(
        "dynamic",
        {
          sensitive: "output",
          retries: {
            limit: 1,
            delay: async ({ ctx, error }) => {
              assert.equal(ctx.attempt, 1);
              assert.equal(error.message, "business failure");
              return "2 seconds";
            },
          },
        },
        (context) => {
          assert.equal(context.config.sensitive, "output");
          assert.equal("delay" in context.config.retries, false);
          throw new Error("business failure");
        },
      );
    }
  }
  const result = await runWorkflow(Flow, {}, {}, event, {
    async claimBatch({ steps }) {
      assert.deepEqual(steps[0].config.retries.delay, { dynamic: true });
      return {
        steps: [
          {
            ordinal: steps[0].ordinal,
            state: "run",
            attempt: 1,
            config: {
              timeout: 60_000,
              sensitive: "output",
              retries: { limit: 1, backoff: "exponential" },
            },
          },
        ],
      };
    },
    async failure(body) {
      failureBody = body;
      const verdict = {
        state: "failed",
        code: "WORKFLOW_STEP_RETRIES_EXHAUSTED",
      };
      finishResult(verdict);
      return verdict;
    },
    async result() {
      return resultReady;
    },
    async drain() {
      return { ok: true };
    },
  });
  assert.equal(result.outcome, "errored");
  assert.equal(result.errorCode, "WORKFLOW_STEP_RETRIES_EXHAUSTED");
  assert.equal(failureBody.resolvedDelayMs, 2000);
});

test("terminate rollback replays completed handlers in LIFO order through durable do steps", async () => {
  const calls = [];
  const descriptors = [];
  const results = new Map();
  const resolved = {
    timeout: 60_000,
    retries: { limit: 5, delay: 10_000, backoff: "exponential" },
  };
  class Flow extends WorkflowEntrypoint {
    async run(_event, step) {
      await step.do(
        "first",
        () => {
          throw new Error("replayed callback executed");
        },
        {
          rollback: async ({ ctx, error, output, stepName }) => {
            calls.push([stepName, ctx.step.name, error.message, output]);
          },
        },
      );
      await step.do(
        "second",
        { timeout: "1 minute" },
        () => {
          throw new Error("replayed callback executed");
        },
        {
          rollback: async ({ ctx, output }) => {
            calls.push([ctx.step.name, output]);
          },
          rollbackConfig: {
            retries: { limit: 1, delay: 0, backoff: "constant" },
          },
        },
      );
      await step.sleep("not-started", 1);
    }
  }
  const rollbackEvent = { ...event, rollback: true };
  const outcome = await runWorkflow(Flow, {}, {}, rollbackEvent, {
    async claimBatch({ steps }) {
      descriptors.push(...steps);
      const item = steps[0];
      if (item.kind === "sleep") {
        return {
          steps: [
            {
              ordinal: item.ordinal,
              state: "rollback_boundary",
              rollbackOrdinal: 2,
            },
          ],
        };
      }
      if (item.ordinal < 2) {
        return {
          steps: [
            {
              ordinal: item.ordinal,
              state: "complete",
              attempt: 1,
              config: resolved,
            },
          ],
        };
      }
      let finish;
      results.set(
        item.ordinal,
        new Promise((resolve) => {
          finish = resolve;
        }),
      );
      results.set(`finish:${item.ordinal}`, finish);
      return {
        steps: [
          { ordinal: item.ordinal, state: "run", attempt: 1, config: resolved },
        ],
      };
    },
    async success({ ordinal, outputBase64 }) {
      const verdict = { state: "complete", outputBase64 };
      results.get(`finish:${ordinal}`)(verdict);
      return verdict;
    },
    async result(ordinal) {
      if (ordinal < 2)
        return {
          state: "complete",
          outputBase64: encodeWorkflowBase64(ordinal + 10),
        };
      return results.get(ordinal);
    },
    async drain() {
      return { ok: true };
    },
  });
  assert.deepEqual(outcome, { outcome: "terminated", finalOrdinal: 4 });
  assert.deepEqual(calls, [
    ["second", 11],
    ["first", "first", "Instance terminated during rollback", 10],
  ]);
  assert.deepEqual(
    descriptors.slice(0, 2).map((item) => item.rollbackConfig),
    [{}, { retries: { limit: 1, delay: 0, backoff: "constant" } }],
  );
  assert.deepEqual(
    descriptors
      .slice(-2)
      .map((item) => [item.ordinal, item.name, item.rollbackConfig]),
    [
      [2, "rollback:1", undefined],
      [3, "rollback:0", undefined],
    ],
  );
});

test("Workflow step permits the official SDK do method proxy", async () => {
  let finishResult;
  const ready = new Promise((resolve) => (finishResult = resolve));
  let callbacks = 0;
  class Flow extends WorkflowEntrypoint {
    async run(_event, step) {
      // The unmodified Python SDK wraps do with a Proxy get trap. A frozen
      // own do property would violate the engine's Proxy invariant here.
      const sdkStep = new Proxy(step, {
        get(target, name) {
          if (name === "do") return (...args) => target.do(...args);
          return Reflect.get(target, name);
        },
      });
      return await sdkStep.do("compute", async () => {
        callbacks++;
        return { value: 5 };
      });
    }
  }
  const result = await runWorkflow(Flow, {}, {}, event, {
    async claimBatch({ steps }) {
      assert.equal(steps.length, 1);
      return { steps: [{ ordinal: 0, state: "run", attempt: 1, config }] };
    },
    async success(body) {
      const reply = { state: "complete", outputBase64: body.outputBase64 };
      finishResult(reply);
      return reply;
    },
    async failure() {
      assert.fail("successful SDK step reported a failure");
    },
    async result() {
      return ready;
    },
    async drain() {
      return { ok: true };
    },
  });
  assert.deepEqual(result, {
    outcome: "complete",
    outputBase64: encodeWorkflowBase64({ value: 5 }),
    finalOrdinal: 1,
  });
  assert.equal(callbacks, 1);
});

test("Workflow SDK non-retryable errors preserve classification without inspecting getters", async () => {
  let getterReads = 0;
  const native = new NonRetryableError("private step detail");
  const named = new Error("private step detail");
  named.name = "NonRetryableError";
  const accessor = new Error("private step detail");
  Object.defineProperty(accessor, "name", {
    get() {
      getterReads++;
      throw new Error("name getter must not run");
    },
  });
  const ordinary = new Error("NonRetryableError: ordinary retryable message");
  class PythonError extends Error {
    type = "NonRetryableError";
  }
  const python = new PythonError("private step detail");
  const unbranded = new Error("private step detail");
  unbranded.type = "NonRetryableError";
  const typeAccessor = new Error("private step detail");
  Object.defineProperty(typeAccessor, "type", {
    get() {
      getterReads++;
      throw new Error("type getter must not run");
    },
  });
  const constructorAccessor = new Error("private step detail");
  constructorAccessor.type = "NonRetryableError";
  Object.setPrototypeOf(
    constructorAccessor,
    Object.create(Error.prototype, {
      constructor: {
        get() {
          getterReads++;
          throw new Error("constructor getter must not run");
        },
      },
    }),
  );
  for (const [error, expected] of [
    [native, "WORKFLOW_NON_RETRYABLE"],
    [named, "WORKFLOW_NON_RETRYABLE"],
    [python, "WORKFLOW_NON_RETRYABLE"],
    [unbranded, "WORKFLOW_EXECUTION_FAILED"],
    [typeAccessor, "WORKFLOW_EXECUTION_FAILED"],
    [constructorAccessor, "WORKFLOW_EXECUTION_FAILED"],
    [accessor, "WORKFLOW_EXECUTION_FAILED"],
    [ordinary, "WORKFLOW_EXECUTION_FAILED"],
    [{ name: "NonRetryableError" }, "WORKFLOW_EXECUTION_FAILED"],
  ]) {
    if (error instanceof Error) {
      for (const key of ["message", "stack", "cause"]) {
        Object.defineProperty(error, key, {
          get() {
            getterReads++;
            throw new Error("exception payload getter must not run");
          },
          configurable: true,
        });
      }
    }
    let finishResult;
    const ready = new Promise((resolve) => (finishResult = resolve));
    const failures = [];
    let callbacks = 0;
    class Flow extends WorkflowEntrypoint {
      async run(_event, step) {
        await step.do("compute", async () => {
          callbacks++;
          throw error;
        });
      }
    }
    const outcome = await runWorkflow(Flow, {}, {}, event, {
      async claimBatch() {
        return { steps: [{ ordinal: 0, state: "run", attempt: 1, config }] };
      },
      async failure(body) {
        failures.push(body);
        const reply = {
          state: "failed",
          code:
            body.code === "WORKFLOW_NON_RETRYABLE"
              ? body.code
              : "WORKFLOW_STEP_RETRIES_EXHAUSTED",
        };
        finishResult(reply);
        return reply;
      },
      async result() {
        return ready;
      },
      async drain() {
        return { ok: true };
      },
    });
    assert.deepEqual(failures, [{ ordinal: 0, code: expected }]);
    assert.equal(outcome.outcome, "errored");
    assert.equal(callbacks, 1);
    assert.equal(getterReads, 0);
  }
});

test("Workflow SDK error roundtrip retains non-retryable verdict and allows catch continuation", async () => {
  class PythonError extends Error {
    type = "NonRetryableError";
  }
  for (const caught of [false, true]) {
    let finishResult;
    const ready = new Promise((resolve) => (finishResult = resolve));
    let callbacks = 0;
    const failures = [];
    class Flow extends WorkflowEntrypoint {
      async run(_event, step) {
        try {
          await step.do("compute", () => {
            callbacks++;
            throw new NonRetryableError("private step detail");
          });
        } catch {
          if (caught) return { caught: true };
          // The SDK converts the JS verdict into a Python exception, losing
          // the WeakMap identity before Pyodide delivers a new PythonError.
          throw new PythonError("private step detail");
        }
      }
    }
    const result = await runWorkflow(Flow, {}, {}, event, {
      async claimBatch() {
        return { steps: [{ ordinal: 0, state: "run", attempt: 1, config }] };
      },
      async failure(body) {
        failures.push(body);
        const reply = { state: "failed", code: body.code };
        finishResult(reply);
        return reply;
      },
      async result() {
        return ready;
      },
      async drain() {
        return { ok: true };
      },
    });
    assert.deepEqual(failures, [
      { ordinal: 0, code: "WORKFLOW_NON_RETRYABLE" },
    ]);
    assert.equal(callbacks, 1);
    assert.deepEqual(
      result,
      caught
        ? {
            outcome: "complete",
            outputBase64: encodeWorkflowBase64({ caught: true }),
            finalOrdinal: 1,
          }
        : {
            outcome: "errored",
            errorCode: "WORKFLOW_NON_RETRYABLE",
            finalOrdinal: 1,
          },
    );
  }
});

test("Workflow context permits SDK waitUntil wrapping and awaits its background work", async () => {
  let completed = false;
  class Flow extends WorkflowEntrypoint {
    constructor(ctx, env) {
      super(ctx, env);
      // workers-runtime-sdk patches this public method during construction.
      const waitUntil = ctx.waitUntil.bind(ctx);
      ctx.waitUntil = (promise) => waitUntil(promise);
    }
    async run() {
      this.ctx.waitUntil(
        Promise.resolve().then(() => {
          completed = true;
        }),
      );
      return 5;
    }
  }
  const ctx = {};
  for (let activation = 0; activation < 2; activation++) {
    completed = false;
    const result = await runWorkflow(Flow, ctx, {}, event, {
      async drain() {
        return { ok: true };
      },
    });
    assert.deepEqual(result, {
      outcome: "complete",
      outputBase64: encodeWorkflowBase64(5),
      finalOrdinal: 0,
    });
    assert.equal(completed, true);
    assert.throws(() => ctx.waitUntil(Promise.resolve()), /WORKFLOW_RUN_STALE/);
  }
});
