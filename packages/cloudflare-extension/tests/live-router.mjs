import assert from "node:assert/strict";
import Cloudflare from "cloudflare";
import { createOpenComputeExtension } from "../src/index.ts";

const baseURL = process.env.OPEN_COMPUTE_V4_BASE_URL;
const apiToken = process.env.OPEN_COMPUTE_V4_TOKEN;
assert.ok(baseURL, "OPEN_COMPUTE_V4_BASE_URL is required");
assert.ok(apiToken, "OPEN_COMPUTE_V4_TOKEN is required");

const client = new Cloudflare({ apiToken, baseURL, maxRetries: 0 });
const accounts = await client.accounts.list();
assert.equal(accounts.result.length, 1);
assert.equal(accounts.result_info.count, 1);
const accountID = accounts.result[0].id;
assert.match(accountID, /^[0-9a-f]{32}$/);
assert.equal((await client.accounts.get({ account_id: accountID })).id, accountID);

const user = await client.user.get();
assert.match(user.id, /^[0-9a-f]{32}$/);
assert.equal((await client.user.tokens.verify()).status, "active");
const memberships = await client.memberships.list();
assert.equal(memberships.result.length, 1);
assert.equal(memberships.result[0].account.id, accountID);

const extension = createOpenComputeExtension(client);
const capabilities = await extension.capabilities.get();
assert.equal(capabilities.wrangler_version, "4.127.1");
assert.equal(capabilities.compatibility_date.minimum, "2026-08-30");
assert.equal(capabilities.compatibility_date.maximum, "2026-08-30");
assert.ok(Object.keys(capabilities.endpoints).length > 0);
const system = await extension.system.status();
assert.equal(typeof system.state, "string");
