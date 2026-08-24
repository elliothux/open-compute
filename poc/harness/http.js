"use strict";

class G0Client {
  constructor(baseUrl) {
    this.baseUrl = baseUrl.replace(/\/$/, "");
  }

  async request(pathname, options = {}) {
    const headers = { ...(options.headers || {}) };
    if (options.body !== undefined && !headers["content-type"]) {
      headers["content-type"] = "application/json";
    }
    const init = {
      method: options.method || (options.body !== undefined ? "POST" : "GET"),
      headers,
      signal: options.signal,
    };
    if (options.body !== undefined) {
      init.body = typeof options.body === "string" ? options.body : JSON.stringify(options.body);
    }
    const res = await fetch(this.baseUrl + pathname, init);
    const text = await res.text();
    let json = null;
    try {
      json = JSON.parse(text);
    } catch {
      json = null;
    }
    return {
      ok: res.ok,
      status: res.status,
      headers: res.headers,
      text,
      json,
    };
  }

  health(options) {
    return this.request("/health", options);
  }

  dispatch(body, headers) {
    return this.request("/g0/dispatch", { body, headers });
  }

  active(body) {
    return this.request("/g0/active", { body });
  }

  route(body) {
    return this.request("/g0/route", { body });
  }

  getRoute() {
    return this.request("/g0/route");
  }

  stats() {
    return this.request("/g0/loader/stats");
  }

  invariant(body) {
    return this.request("/g0/loader/invariant", { body });
  }

  doOp(body) {
    return this.request("/g0/do", { body });
  }

  fault(body) {
    return this.request("/g0/fault", { body });
  }
}

async function waitForHealth(client, options = {}) {
  const timeoutMs = typeof options === "number" ? options : options.timeoutMs ?? 15000;
  const signal = typeof options === "number" ? undefined : options.signal;
  const start = Date.now();
  let lastErr = null;
  while (Date.now() - start < timeoutMs) {
    if (signal?.aborted) {
      throw new Error(
        `workerd exited before health: ${lastErr && lastErr.message ? lastErr.message : "aborted"}`
      );
    }
    try {
      const res = await client.health({ signal });
      if (res.ok && res.json?.ok) return res;
      lastErr = new Error(`health ${res.status}`);
    } catch (err) {
      if (err && (err.name === "AbortError" || signal?.aborted)) {
        throw new Error(
          `workerd exited before health: ${lastErr && lastErr.message ? lastErr.message : err.message}`
        );
      }
      lastErr = err;
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  throw new Error(`workerd was not ready within ${timeoutMs}ms: ${lastErr && lastErr.message}`);
}

module.exports = { G0Client, waitForHealth };
