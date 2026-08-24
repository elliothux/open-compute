import { DurableObject } from "cloudflare:workers";

function readValue(sql) {
  const rows = sql.exec("SELECT value FROM counter WHERE id = 1").toArray();
  return rows.length === 0 ? 0 : Number(rows[0].value);
}

function ensureCounter(sql) {
  sql.exec(`
    CREATE TABLE IF NOT EXISTS counter (
      id INTEGER PRIMARY KEY CHECK (id = 1),
      value INTEGER NOT NULL
    )
  `);
}

function incrementCounter(sql) {
  sql.exec(`
    INSERT INTO counter (id, value) VALUES (1, 1)
    ON CONFLICT(id) DO UPDATE SET value = value + 1
  `);
  return readValue(sql);
}

function listSqlTables(sql) {
  return sql
    .exec("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
    .toArray()
    .map((row) => row.name);
}

function readSupervisorSecret(sql) {
  try {
    const rows = sql.exec("SELECT v FROM supervisor_private").toArray();
    return { visible: true, rows };
  } catch {
    return { visible: false, error: "not-visible" };
  }
}

export class Counter extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.jsNonce = crypto.randomUUID();
    this.jsTicks = 0;
    ensureCounter(this.ctx.storage.sql);
  }

  async fetch(request) {
    const url = request && request.url ? new URL(request.url) : null;
    const hold = url ? url.searchParams.get("hold") : null;
    const ms = url ? Number(url.searchParams.get("ms") || "0") : 0;
    const holdMs = Number.isFinite(ms) && ms > 0 ? Math.min(ms, 15000) : 0;

    if (hold === "before-write" && holdMs > 0) {
      await scheduler.wait(holdMs);
    }

    const value = this.ctx.storage.transactionSync(() => {
      if (hold === "before-commit" && holdMs > 0) {
        const started = Date.now();
        while (Date.now() - started < holdMs) {
          /* keep the uncommitted SQL transaction open */
        }
      }
      return incrementCounter(this.ctx.storage.sql);
    });
    this.jsTicks += 1;
    await this.ctx.storage.sync();
    if (hold === "after-write" && holdMs > 0) {
      await scheduler.wait(holdMs);
    }
    if (this.ctx.props?.g0Fault === "F7") {
      throw new Error("FAULT_INJECTED:F7");
    }
    return Response.json({
      value,
      codeVersion: "A",
      jsNonce: this.jsNonce,
      jsTicks: this.jsTicks,
    });
  }

  async getValue() {
    return {
      value: readValue(this.ctx.storage.sql),
      codeVersion: "A",
      jsNonce: this.jsNonce,
      jsTicks: this.jsTicks,
    };
  }

  async failAfterWrite() {
    const before = readValue(this.ctx.storage.sql);
    try {
      this.ctx.storage.transactionSync(() => {
        incrementCounter(this.ctx.storage.sql);
        throw new Error("g0-fail-after-write");
      });
    } catch {
      return {
        threw: true,
        before,
        after: readValue(this.ctx.storage.sql),
        codeVersion: "A",
        message: "g0-fail-after-write",
        jsNonce: this.jsNonce,
      };
    }
    return {
      threw: false,
      before,
      after: readValue(this.ctx.storage.sql),
      codeVersion: "A",
      jsNonce: this.jsNonce,
    };
  }

  async getIdentity() {
    return {
      id: String(this.ctx.id),
      name: this.ctx.id?.name ?? null,
      codeVersion: "A",
      jsNonce: this.jsNonce,
      jsTicks: this.jsTicks,
    };
  }

  async listTables() {
    return listSqlTables(this.ctx.storage.sql);
  }

  async readSupervisorSecret() {
    return readSupervisorSecret(this.ctx.storage.sql);
  }
}

export class AltCounter extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.jsNonce = crypto.randomUUID();
    this.jsTicks = 0;
    ensureCounter(this.ctx.storage.sql);
  }

  async fetch() {
    const value = this.ctx.storage.transactionSync(() => incrementCounter(this.ctx.storage.sql));
    this.jsTicks += 1;
    await this.ctx.storage.sync();
    return Response.json({
      value,
      codeVersion: "alt",
      jsNonce: this.jsNonce,
      jsTicks: this.jsTicks,
    });
  }

  async getValue() {
    return {
      value: readValue(this.ctx.storage.sql),
      codeVersion: "alt",
      jsNonce: this.jsNonce,
      jsTicks: this.jsTicks,
    };
  }

  async failAfterWrite() {
    const before = readValue(this.ctx.storage.sql);
    try {
      this.ctx.storage.transactionSync(() => {
        incrementCounter(this.ctx.storage.sql);
        throw new Error("g0-fail-after-write");
      });
    } catch {
      return {
        threw: true,
        before,
        after: readValue(this.ctx.storage.sql),
        codeVersion: "alt",
        message: "g0-fail-after-write",
        jsNonce: this.jsNonce,
      };
    }
    return {
      threw: false,
      before,
      after: readValue(this.ctx.storage.sql),
      codeVersion: "alt",
      jsNonce: this.jsNonce,
    };
  }

  async getIdentity() {
    return {
      id: String(this.ctx.id),
      name: this.ctx.id?.name ?? null,
      codeVersion: "alt",
      jsNonce: this.jsNonce,
      jsTicks: this.jsTicks,
    };
  }

  async listTables() {
    return listSqlTables(this.ctx.storage.sql);
  }

  async readSupervisorSecret() {
    return readSupervisorSecret(this.ctx.storage.sql);
  }
}
