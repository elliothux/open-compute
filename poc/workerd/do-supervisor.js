import { DurableObject } from "cloudflare:workers";
import { assembleWorkerCode } from "./code.js";
import { tenantError, classifyThrown } from "./errors.js";
import { logEvent, requestIdFrom, sha256hex } from "./log.js";
import { getDeployment, loaderKey } from "./registry.js";

const IDENT = /^[A-Za-z0-9_.:-]{1,64}$/;
const FACET_NAME_MAX = 240;
const assembling = new Map();
const callbackCounts = new Map();
let faults = {};

function wellFormed(value) {
  if (typeof value !== "string" || value.length === 0) return false;
  if (typeof value.isWellFormed === "function" && !value.isWellFormed()) return false;
  if (/[\uD800-\uDFFF]/.test(value)) return false;
  return true;
}

function invalidIdentifier() {
  const err = new Error("IDENTIFIER_INVALID");
  err.errorCode = "IDENTIFIER_INVALID";
  throw err;
}

function encodeFacetName(doStorageId, className, objectId) {
  for (const value of [doStorageId, className, objectId]) {
    if (!wellFormed(value) || !IDENT.test(value)) invalidIdentifier();
  }
  const name = `v1/s/${doStorageId}/c/${className}/o/${objectId}`;
  if (name.length > FACET_NAME_MAX) invalidIdentifier();
  const decoded = decodeFacetName(name);
  if (
    !decoded ||
    decoded.doStorageId !== doStorageId ||
    decoded.className !== className ||
    decoded.objectId !== objectId
  ) {
    invalidIdentifier();
  }
  return name;
}

function decodeFacetName(name) {
  if (typeof name !== "string") return null;
  const match = /^v1\/s\/([^/]+)\/c\/([^/]+)\/o\/([^/]+)$/.exec(name);
  if (!match) return null;
  return { doStorageId: match[1], className: match[2], objectId: match[3] };
}

function requireDeployment(accountId, workerId, deploymentId) {
  if (!wellFormed(accountId) || !wellFormed(workerId) || !wellFormed(deploymentId)) {
    const err = new Error("DEPLOYMENT_NOT_FOUND");
    err.errorCode = "DEPLOYMENT_NOT_FOUND";
    throw err;
  }
  const deploymentKey = loaderKey(accountId, workerId, deploymentId);
  const spec = getDeployment(deploymentKey);
  if (!spec) {
    const err = new Error("DEPLOYMENT_NOT_FOUND");
    err.errorCode = "DEPLOYMENT_NOT_FOUND";
    throw err;
  }
  return { deploymentKey, spec };
}

function assembleOnce(env, key, spec) {
  const existing = assembling.get(key);
  if (existing) return existing;
  const pending = assembleWorkerCode(env, key, {
    extraEnv: {
      G0_IDENTITY: {
        accountId: spec.accountId,
        workerId: spec.workerId,
        deploymentId: spec.deploymentId,
      },
    },
  }).finally(() => {
    if (assembling.get(key) === pending) assembling.delete(key);
  });
  assembling.set(key, pending);
  return pending;
}

function classifyDoError(err) {
  const classified = classifyThrown(err);
  if (classified.errorCode === "LOADER_ERROR") {
    return { errorCode: "DO_ERROR", status: classified.status || 500 };
  }
  return classified;
}

function isInternalWorkerdError(err) {
  const message = String(err && err.message ? err.message : err);
  return /internal error/i.test(message) || /ActorClassChannel is not ready/i.test(message);
}

function classNotFound() {
  const err = new Error("CLASS_NOT_FOUND");
  err.errorCode = "CLASS_NOT_FOUND";
  return err;
}

async function maybeAwait(value) {
  if (value != null && typeof value.then === "function") return value;
  return value;
}

function incrementUrl(body) {
  const hold = body && typeof body.hold === "string" ? body.hold : "";
  const holdMs = Number(body && body.holdMs);
  const allowed =
    hold === "before-write" || hold === "before-commit" || hold === "after-write";
  if (!allowed || !Number.isFinite(holdMs) || holdMs <= 0 || holdMs > 15000) {
    return "https://g0.invalid/increment";
  }
  return `https://g0.invalid/increment?hold=${encodeURIComponent(hold)}&ms=${encodeURIComponent(
    String(Math.floor(holdMs))
  )}`;
}

function moduleExportsDoClass(modules, className) {
  if (!wellFormed(className) || !IDENT.test(className)) return false;
  const exportedClass = new RegExp(`export\\s+class\\s+${className}\\b`);
  const exportedAs = new RegExp(`\\bas\\s+${className}\\b`);
  const exportedNamed = new RegExp(`export\\s*\\{[^}]*\\b${className}\\b[^}]*\\}`);
  for (const source of Object.values(modules || {})) {
    if (typeof source !== "string") continue;
    if (exportedClass.test(source) || exportedAs.test(source) || exportedNamed.test(source)) {
      return true;
    }
  }
  return false;
}

export class DoSupervisor extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.ctx.storage.sql.exec(`
      CREATE TABLE IF NOT EXISTS supervisor_private (
        k TEXT PRIMARY KEY,
        v TEXT NOT NULL
      )
    `);
    this.ctx.storage.sql.exec(
      `INSERT OR IGNORE INTO supervisor_private (k, v) VALUES ('secret', 'supervisor-only')`
    );
    this.ctx.storage.sql.exec(`
      CREATE TABLE IF NOT EXISTS supervisor_meta (
        k TEXT PRIMARY KEY,
        v TEXT NOT NULL
      )
    `);
    this.ctx.storage.sql.exec(
      `INSERT OR REPLACE INTO supervisor_meta (k, v) VALUES ('alive', '1')`
    );
  }

  async #worker(deploymentKey, spec) {
    const assembled = await assembleOnce(this.env, deploymentKey, spec);
    return this.env.LOADER.get(deploymentKey, () => {
      callbackCounts.set(deploymentKey, (callbackCounts.get(deploymentKey) ?? 0) + 1);
      return assembled.code;
    });
  }

  async #assertExportedDoClass(worker, className) {
    try {
      await worker.getEntrypoint(className).fetch("https://g0.invalid/g0-class-probe");
    } catch (err) {
      const message = String(err && err.message ? err.message : err);
      if (
        /no such entrypoint/i.test(message) ||
        /does not export an entrypoint named/i.test(message) ||
        /does not export a Durable Object class named/i.test(message)
      ) {
        throw classNotFound();
      }
    }
  }

  async #loadClass(deploymentKey, spec, className, props) {
    const assembled = await assembleOnce(this.env, deploymentKey, spec);
    // Stock getDurableObjectClass is lazy: a missing class makes facets.get throw
    // an internal ActorClassChannel error instead of a tenant-safe CLASS_NOT_FOUND.
    if (!moduleExportsDoClass(assembled.code.modules, className)) {
      throw classNotFound();
    }
    const worker = this.env.LOADER.get(deploymentKey, () => {
      callbackCounts.set(deploymentKey, (callbackCounts.get(deploymentKey) ?? 0) + 1);
      return assembled.code;
    });
    try {
      return worker.getDurableObjectClass(className, props ? { props } : undefined);
    } catch (err) {
      const classified = classifyThrown(err);
      if (classified.errorCode === "CLASS_NOT_FOUND") {
        throw classNotFound();
      }
      throw err;
    }
  }

  async #facet(body) {
    const { doStorageId, className, objectId, accountId, workerId, deploymentId } = body;
    const facetName = encodeFacetName(doStorageId, className, objectId);
    const { deploymentKey, spec } = requireDeployment(accountId, workerId, deploymentId);
    if (faults.F11) {
      throw new Error("FAULT_INJECTED:F11");
    }
    const cls = await this.#loadClass(deploymentKey, spec, className, {
      g0Fault: faults.F7 ? "F7" : null,
    });
    try {
      const facet = this.ctx.facets.get(facetName, () => ({
        class: cls,
        id: objectId,
      }));
      return { facet, facetName, deploymentKey };
    } catch (err) {
      if (isInternalWorkerdError(err)) {
        const worker = await this.#worker(deploymentKey, spec);
        await this.#assertExportedDoClass(worker, className);
      }
      throw err;
    }
  }

  async #log(fields) {
    const body = fields.body || {};
    logEvent({
      requestId: fields.requestId,
      deploymentId: body.deploymentId ?? null,
      loaderKeyHash:
        body.accountId && body.workerId && body.deploymentId
          ? await sha256hex(loaderKey(body.accountId, body.workerId, body.deploymentId))
          : null,
      doStorageIdHash: wellFormed(body.doStorageId) ? await sha256hex(body.doStorageId) : null,
      className: wellFormed(body.className) ? body.className : null,
      objectIdHash: wellFormed(body.objectId) ? await sha256hex(body.objectId) : null,
      dispatchKind: fields.dispatchKind,
      durationMs: fields.durationMs,
      outcome: fields.outcome,
      errorCode: fields.errorCode ?? null,
      extra: { op: fields.op },
    });
  }

  async fetch(request) {
    const requestId = requestIdFrom(request);
    const started = Date.now();
    const url = new URL(request.url);
    let body = {};
    if (request.method !== "GET") {
      body = await request.json().catch(() => ({}));
    }
    const op = body.op || url.searchParams.get("op");
    try {
      this.ctx.storage.sql.exec(
        `INSERT OR REPLACE INTO supervisor_meta (k, v) VALUES ('alive', '1')`
      );

      if (op === "increment") {
        const { facet, facetName } = await this.#facet(body);
        const resp = await facet.fetch(incrementUrl(body));
        const payload = await resp.json();
        await this.#log({
          requestId,
          body,
          op,
          dispatchKind: "do-fetch",
          durationMs: Date.now() - started,
          outcome: "ok",
        });
        return Response.json({ ok: true, ...payload, facetName });
      }

      if (op === "getValue") {
        const { facet, facetName } = await this.#facet(body);
        const value = await facet.getValue();
        await this.#log({
          requestId,
          body,
          op,
          dispatchKind: "do-rpc",
          durationMs: Date.now() - started,
          outcome: "ok",
        });
        return Response.json({ ok: true, ...value, facetName });
      }

      if (op === "failAfterWrite") {
        const { facet, facetName } = await this.#facet(body);
        const result = await facet.failAfterWrite();
        await this.#log({
          requestId,
          body,
          op,
          dispatchKind: "do-rpc",
          durationMs: Date.now() - started,
          outcome: "ok",
        });
        return Response.json({
          ok: true,
          ...result,
          facetName,
          classification: result.after === result.before ? "not-applied" : "applied",
        });
      }

      if (op === "getIdentity") {
        const { facet, facetName } = await this.#facet(body);
        const identity = await facet.getIdentity();
        await this.#log({
          requestId,
          body,
          op,
          dispatchKind: "do-rpc",
          durationMs: Date.now() - started,
          outcome: "ok",
        });
        return Response.json({ ok: true, identity, facetName });
      }

      if (op === "abort") {
        if (faults.F10) {
          throw new Error("FAULT_INJECTED:F10");
        }
        const facetName = encodeFacetName(body.doStorageId, body.className, body.objectId);
        if (typeof this.ctx.facets.abort !== "function") {
          throw new Error("native ctx.facets.abort is unavailable");
        }
        const reason = body.reason || "g0-code-restart";
        await maybeAwait(this.ctx.facets.abort(facetName, reason));
        await this.#log({
          requestId,
          body,
          op,
          dispatchKind: "do",
          durationMs: Date.now() - started,
          outcome: "ok",
        });
        return Response.json({ ok: true, facetName, aborted: true, reason });
      }

      if (op === "delete") {
        const facetName = encodeFacetName(body.doStorageId, body.className, body.objectId);
        if (typeof this.ctx.facets.delete !== "function") {
          throw new Error("native ctx.facets.delete is unavailable");
        }
        await maybeAwait(this.ctx.facets.delete(facetName));
        await this.#log({
          requestId,
          body,
          op,
          dispatchKind: "do",
          durationMs: Date.now() - started,
          outcome: "ok",
        });
        return Response.json({ ok: true, facetName, deleted: true });
      }

      if (op === "stampSupervisor") {
        const key = body.key;
        const value = body.value;
        if (!wellFormed(key) || !IDENT.test(key) || !wellFormed(value) || !IDENT.test(value)) {
          invalidIdentifier();
        }
        this.ctx.storage.sql.exec(
          `INSERT OR REPLACE INTO supervisor_meta (k, v) VALUES (?, ?)`,
          key,
          value
        );
        const row = this.ctx.storage.sql.exec(`SELECT v FROM supervisor_meta WHERE k = ?`, key).one();
        await this.#log({
          requestId,
          body,
          op,
          dispatchKind: "do",
          durationMs: Date.now() - started,
          outcome: "ok",
        });
        return Response.json({ ok: true, key, value: row.v });
      }

      if (op === "probeSupervisor") {
        const tables = this.ctx.storage.sql
          .exec("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
          .toArray()
          .map((row) => row.name);
        const privateProbe = this.ctx.storage.sql
          .exec(
            "SELECT COUNT(*) AS n FROM supervisor_private WHERE k = 'secret' AND length(v) > 0"
          )
          .one();
        const privateValuePresent = Number(privateProbe.n) > 0;
        const meta = this.ctx.storage.sql
          .exec("SELECT k, v FROM supervisor_meta")
          .toArray();
        let counterVisible = false;
        try {
          this.ctx.storage.sql.exec("SELECT value FROM counter WHERE id = 1").toArray();
          counterVisible = true;
        } catch {
          counterVisible = false;
        }
        await this.#log({
          requestId,
          body,
          op,
          dispatchKind: "do",
          durationMs: Date.now() - started,
          outcome: "ok",
        });
        return Response.json({
          ok: true,
          tables,
          privateValuePresent,
          meta,
          counterVisible,
        });
      }

      if (op === "probeFacet") {
        const { facet, facetName } = await this.#facet(body);
        const tables = await facet.listTables();
        const supervisorSecret = await facet.readSupervisorSecret();
        await this.#log({
          requestId,
          body,
          op,
          dispatchKind: "do-rpc",
          durationMs: Date.now() - started,
          outcome: "ok",
        });
        return Response.json({ ok: true, facetName, tables, supervisorSecret });
      }

      if (op === "stats") {
        return Response.json({
          ok: true,
          callbacks: Object.fromEntries(callbackCounts),
          faults,
        });
      }

      if (op === "fault") {
        faults[body.point] = Boolean(body.enabled);
        return Response.json({ ok: true, faults });
      }

      await this.#log({
        requestId,
        body,
        op,
        dispatchKind: "do",
        durationMs: Date.now() - started,
        outcome: "error",
        errorCode: "DO_ERROR",
      });
      return tenantError("DO_ERROR", requestId, body.deploymentId, 404);
    } catch (err) {
      const classified = classifyDoError(err);
      await this.#log({
        requestId,
        body,
        op,
        dispatchKind: "do",
        durationMs: Date.now() - started,
        outcome: "error",
        errorCode: classified.errorCode,
      });
      return tenantError(
        classified.errorCode,
        requestId,
        body.deploymentId ?? null,
        classified.status
      );
    }
  }
}

export default {
  async fetch(request, env) {
    const id = env.DoSupervisor.idFromName("g0-supervisor");
    const stub = env.DoSupervisor.get(id);
    return stub.fetch(request);
  },
};
