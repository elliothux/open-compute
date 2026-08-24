function describeValue(value) {
  if (value === null) return { type: "null", value: null };
  if (value === undefined) return { type: "undefined" };
  const type = typeof value;
  if (type === "string" || type === "number" || type === "boolean") {
    return { type, value };
  }
  if (type === "bigint") return { type, value: String(value) };
  if (type === "symbol") return { type, ctor: "Symbol" };
  if (type === "function") return { type, ctor: "Function" };
  return { type, ctor: value?.constructor?.name ?? null };
}

function errorInfo(err) {
  return {
    name: err?.name ?? "Error",
    message: String(err?.message || err),
    stack: err?.stack ?? null,
    errorCode: err?.errorCode ?? null,
  };
}

function hasEnv(env, name) {
  try {
    return env[name] != null;
  } catch {
    return false;
  }
}

function methodType(obj, name) {
  try {
    return typeof obj?.[name];
  } catch {
    return "throw";
  }
}

async function callMethod(obj, name, args = []) {
  try {
    const result = await obj[name](...args);
    let text = null;
    if (result && typeof result === "object" && typeof result.text === "function") {
      try {
        text = await result.text();
      } catch {
        text = null;
      }
    }
    return { ok: true, result: describeValue(result), status: result?.status ?? null, text };
  } catch (err) {
    return { ok: false, ...errorInfo(err) };
  }
}

const CLONE_SAMPLES = [
  { name: "string", value: "clone-ok" },
  { name: "empty-string", value: "" },
  { name: "number", value: 42 },
  { name: "boolean", value: true },
  { name: "null", value: null },
  { name: "object", value: { resourceId: "kv_fixture_b", path: "/var/g0-data/do/secret.sqlite" } },
  { name: "array", value: ["A", "B"] },
  { name: "undefined", value: undefined },
  { name: "function", value: () => "nope" },
  { name: "symbol", value: Symbol("x") },
  { name: "bigint", value: 1n },
  { name: "date", value: new Date("2026-08-23T00:00:00.000Z") },
  { name: "map", value: new Map([["shared", "B"]]) },
  { name: "bytes", value: new Uint8Array([1, 2, 3]) },
];

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const body = await request.json().catch(() => ({}));
    const kv = env.KV;

    if (!kv) {
      return Response.json(
        {
          ok: false,
          errorCode: "KV_MISSING",
          envKeys: Object.keys(env ?? {}),
        },
        { status: 500 }
      );
    }

    if (url.pathname === "/get") {
      try {
        const value = await kv.get(body.key);
        return Response.json({ ok: true, value, claimed: body.resourceId ?? null });
      } catch (err) {
        return Response.json({ ok: false, ...errorInfo(err) }, { status: 500 });
      }
    }

    if (url.pathname === "/put") {
      try {
        await kv.put(body.key, body.value);
        return Response.json({ ok: true });
      } catch (err) {
        return Response.json({ ok: false, ...errorInfo(err) }, { status: 500 });
      }
    }

    if (url.pathname === "/forge") {
      const attempts = [];
      try {
        attempts.push({
          via: "second-arg",
          value: await kv.get(body.key ?? "shared", { resourceId: "kv_fixture_b" }),
        });
      } catch (err) {
        attempts.push({ via: "second-arg", ...errorInfo(err) });
      }
      try {
        attempts.push({
          via: "third-arg-put-ignored",
          put: await kv.put("forge-claim", "from-a", { resourceId: "kv_fixture_b" }),
          value: await kv.get("forge-claim"),
        });
      } catch (err) {
        attempts.push({ via: "third-arg-put-ignored", ...errorInfo(err) });
      }
      try {
        attempts.push({
          via: "key-path",
          value: await kv.get("/tmp/secret-or-other-resource"),
        });
      } catch (err) {
        attempts.push({ via: "key-path", ...errorInfo(err) });
      }
      try {
        attempts.push({
          via: "internal-url-key",
          value: await kv.get("http://binding-host/admin"),
        });
      } catch (err) {
        attempts.push({ via: "internal-url-key", ...errorInfo(err) });
      }
      try {
        attempts.push({
          via: "other-resource-id-key",
          value: await kv.get("kv_fixture_b"),
        });
      } catch (err) {
        attempts.push({ via: "other-resource-id-key", ...errorInfo(err) });
      }
      try {
        await kv.put("/etc/passwd", "http://127.0.0.1/internal");
        attempts.push({
          via: "path-url-as-data",
          value: await kv.get("/etc/passwd"),
        });
      } catch (err) {
        attempts.push({ via: "path-url-as-data", ...errorInfo(err) });
      }
      let propsMutate = null;
      try {
        if (kv.ctx && kv.ctx.props) {
          kv.ctx.props.resourceId = "kv_fixture_b";
          propsMutate = { mutated: true, resourceId: kv.ctx.props.resourceId };
        } else {
          propsMutate = { mutated: false, visible: false };
        }
      } catch (err) {
        propsMutate = { mutated: false, ...errorInfo(err) };
      }
      return Response.json({
        ok: true,
        attempts,
        propsMutate,
        requestResourceHeader: request.headers.get("x-resource-id"),
        envIdentity: env.G0_IDENTITY ?? null,
      });
    }

    if (url.pathname === "/probe") {
      const kvKeys = [];
      try {
        kvKeys.push(...Object.keys(kv ?? {}));
      } catch {
        kvKeys.push("unenumerable");
      }
      const calls = {
        list: await callMethod(kv, "list"),
        admin: await callMethod(kv, "admin"),
        openFile: await callMethod(kv, "openFile"),
        listResources: await callMethod(kv, "listResources"),
        stats: await callMethod(kv, "stats"),
        setFault: await callMethod(kv, "setFault"),
        selectResource: await callMethod(kv, "selectResource"),
        dump: await callMethod(kv, "dump"),
        connect: await callMethod(kv, "connect"),
      };
      return Response.json({
        envKeys: Object.keys(env ?? {}).sort(),
        kvKeys,
        hasKV: kv != null,
        methodTypes: {
          list: methodType(kv, "list"),
          admin: methodType(kv, "admin"),
          openFile: methodType(kv, "openFile"),
          listResources: methodType(kv, "listResources"),
          fetch: methodType(kv, "fetch"),
          connect: methodType(kv, "connect"),
          stats: methodType(kv, "stats"),
          setFault: methodType(kv, "setFault"),
          selectResource: methodType(kv, "selectResource"),
          get: methodType(kv, "get"),
          put: methodType(kv, "put"),
        },
        calls,
        hasBackend: hasEnv(env, "BINDING_BACKEND"),
        hasBindingHost: hasEnv(env, "BINDING_HOST"),
        hasLoader: hasEnv(env, "LOADER"),
        hasFixtures: hasEnv(env, "FIXTURES"),
        hasEcho: hasEnv(env, "ECHO"),
        identityKeys: Object.keys(env.G0_IDENTITY ?? {}).sort(),
        identity: env.G0_IDENTITY ?? null,
      });
    }

    if (url.pathname === "/clone") {
      const priorKey = "clone-prior";
      const tryKey = "clone-try";
      const priorValue = "prior";
      try {
        await kv.put(priorKey, priorValue);
      } catch (err) {
        return Response.json({ ok: false, stage: "prior-put", ...errorInfo(err) }, { status: 500 });
      }
      const results = [];
      const samples = body.useBodySamples ? body.samples ?? [] : CLONE_SAMPLES;
      for (const sample of samples) {
        const input = sample && typeof sample === "object" && "value" in sample ? sample.value : sample;
        const name = sample && typeof sample === "object" && sample.name ? sample.name : describeValue(input).type;
        try {
          await kv.put(tryKey, input);
          const got = await kv.get(tryKey);
          results.push({
            ok: true,
            name,
            input: describeValue(input),
            output: describeValue(got),
          });
        } catch (err) {
          results.push({
            ok: false,
            name,
            input: describeValue(input),
            error: errorInfo(err),
          });
        }
      }
      let priorAfter = null;
      let tryAfter = null;
      try {
        priorAfter = await kv.get(priorKey);
        tryAfter = await kv.get(tryKey);
      } catch (err) {
        priorAfter = { error: errorInfo(err) };
      }
      return Response.json({ ok: true, results, priorAfter, tryAfter });
    }

    if (url.pathname === "/error") {
      try {
        await kv.get("__g0_internal_error");
        return Response.json({ ok: true, unexpected: true });
      } catch (err) {
        return Response.json({
          ok: false,
          ...errorInfo(err),
        });
      }
    }

    if (url.pathname === "/fetch-kv") {
      try {
        const resp = await kv.fetch("https://example.com/admin");
        return Response.json({
          ok: true,
          status: resp.status,
          text: await resp.text(),
        });
      } catch (err) {
        return Response.json({
          ok: false,
          ...errorInfo(err),
        });
      }
    }

    return Response.json({ ok: false, errorCode: "UNKNOWN_BINDING_OP" }, { status: 404 });
  },
};
