import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "./compiled-runtime.mjs";

const { ArtifactError, ArtifactsBinding } = await importRuntime(
  "artifacts/facade.ts",
);

const info = {
  id: "repo-id",
  name: "source",
  description: null,
  defaultBranch: "main",
  createdAt: "2026-09-10T00:00:00Z",
  updatedAt: "2026-09-10T00:00:00Z",
  lastPushAt: null,
  source: null,
  readOnly: false,
  remote: "https://artifacts.example.test/git/apps/source.git",
};
const created = {
  id: "repo-id",
  name: "source",
  description: null,
  defaultBranch: "main",
  remote: info.remote,
  token: "once",
  tokenExpiresAt: "2026-09-11T00:00:00Z",
};

test("Artifacts facade exposes the pinned namespace and repository methods", async () => {
  const calls = [];
  const binding = new ArtifactsBinding({
    async call(operation, input) {
      calls.push({ operation, input });
      if (operation === "get") return info;
      if (operation === "list") {
        const { remote: _, ...listed } = info;
        return { repos: [listed], total: 1, cursor: "next" };
      }
      if (operation === "create-token") {
        return {
          id: "token-id",
          plaintext: "once",
          scope: "read",
          expiresAt: "2026-09-10T00:01:00Z",
        };
      }
      if (operation === "list-tokens") {
        return {
          tokens: [
            {
              id: "token-id",
              scope: "read",
              state: "active",
              createdAt: "2026-09-10T00:00:00Z",
              expiresAt: "2026-09-10T00:01:00Z",
            },
          ],
          total: 1,
        };
      }
      if (operation === "revoke-token" || operation === "delete") return true;
      return created;
    },
  });

  assert.deepEqual(await binding.create("source"), created);
  const repository = await binding.get("source");
  assert.equal(repository.remote, info.remote);
  assert.equal((await repository.createToken("read", 60)).scope, "read");
  assert.equal((await repository.listTokens()).total, 1);
  assert.equal(await repository.revokeToken("token-id"), true);
  assert.deepEqual(await repository.fork("target"), created);
  assert.deepEqual(
    await binding.import({
      source: { url: "https://example.com/source.git" },
      target: { name: "imported" },
    }),
    created,
  );
  const { remote: _, ...listedInfo } = info;
  assert.deepEqual(await binding.list({ limit: 1 }), {
    repos: [listedInfo],
    total: 1,
    cursor: "next",
  });
  assert.equal(await binding.delete("source"), true);
  assert.deepEqual(
    calls.map(({ operation }) => operation),
    [
      "create",
      "get",
      "create-token",
      "list-tokens",
      "revoke-token",
      "fork",
      "import",
      "list",
      "delete",
    ],
  );
});

test("Artifacts facade preserves stable errors and rejects malformed success", async () => {
  const expected = new ArtifactError("INVALID_TTL", 10103);
  const failed = new ArtifactsBinding({
    call() {
      throw expected;
    },
  });
  await assert.rejects(failed.create("repo"), (error) => {
    assert.equal(error.name, "ArtifactsError");
    assert.equal(error.code, "INVALID_TTL");
    assert.equal(error.numericCode, 10103);
    return true;
  });
  const malformed = new ArtifactsBinding({ call: async () => ({ total: 1 }) });
  await assert.rejects(malformed.list(), {
    name: "ArtifactsError",
    message: "INTERNAL_ERROR",
  });
});
