import { expect, test } from "bun:test";
import { createOpenComputeClient } from "@open-compute/sdk";
import {
  saveWorkerResourceBinding,
  type ResourceBindingChange,
} from "../../src/components/worker-resource-binding-save";

function fixture(bindings: unknown[], ids = ["old", "new"]) {
  const requests: Request[] = [];
  const results = [
    { items: [{ id: "old" }] },
    { items: [] },
    { bindings },
    { items: ids.map((id) => ({ id })) },
    { items: [] },
    { id: "deployment" },
  ];
  const client = createOpenComputeClient({
    apiToken: "test-token",
    baseURL: "https://compute.example/client/v4",
    maxRetries: 0,
    fetch: async (input, init) => {
      requests.push(
        input instanceof Request ? input.clone() : new Request(input, init),
      );
      if (results.length === 0) throw new Error("Unexpected API request");
      return Response.json({
        success: true,
        result: results.shift(),
        errors: [],
        messages: [],
        result_info: { page: 1, total_pages: 1 },
      });
    },
  });
  return { client, requests };
}

test("Browser bindings add, rename and delete through verified immutable Versions", async () => {
  const cases: {
    names: string[];
    change: ResourceBindingChange;
    expected: { type: string; name: string }[];
  }[] = [
    {
      names: ["CONFIG"],
      change: { type: "browser", originalName: null, name: " BROWSER " },
      expected: [
        { type: "inherit", name: "CONFIG" },
        { type: "browser", name: "BROWSER" },
      ],
    },
    {
      names: ["CONFIG", "BROWSER"],
      change: { type: "browser", originalName: "BROWSER", name: "RENDER" },
      expected: [
        { type: "inherit", name: "CONFIG" },
        { type: "browser", name: "RENDER" },
      ],
    },
    {
      names: ["CONFIG", "BROWSER"],
      change: { type: "browser", originalName: "BROWSER", remove: true },
      expected: [{ type: "inherit", name: "CONFIG" }],
    },
  ];
  for (const { names, change, expected } of cases) {
    const { client, requests } = fixture(expected);
    await saveWorkerResourceBinding(client, "account", "worker", names, change);
    expect(requests.map((request) => request.method)).toEqual([
      "GET",
      "GET",
      "PATCH",
      "GET",
      "GET",
      "POST",
    ]);
    for (const request of requests) {
      expect(new URL(request.url).pathname).toStartWith(
        "/client/v4/accounts/account/workers/scripts/worker/",
      );
      expect(request.headers.get("authorization")).toBe("Bearer test-token");
    }
    const part = (await requests[2]!.formData()).get("settings");
    if (!(part instanceof Blob)) throw new Error("Missing settings JSON part");
    const settings: unknown = JSON.parse(await part.text());
    expect(settings).toMatchObject({ bindings: expected });
    expect(await requests[5]!.json()).toMatchObject({
      strategy: "percentage",
      versions: [{ version_id: "new", percentage: 100 }],
    });
  }
});

test("Browser binding mismatches and ambiguous Versions never deploy", async () => {
  for (const { bindings, ids, error } of [
    { bindings: [], ids: ["old", "new"], error: "does not match" },
    {
      bindings: [{ type: "browser", name: "BROWSER" }],
      ids: ["old", "new", "other"],
      error: "identified uniquely",
    },
  ]) {
    const { client, requests } = fixture(bindings, ids);
    await expect(
      saveWorkerResourceBinding(client, "account", "worker", [], {
        type: "browser",
        originalName: null,
        name: "BROWSER",
      }),
    ).rejects.toThrow(error);
    expect(requests.map((request) => request.method)).toEqual([
      "GET",
      "GET",
      "PATCH",
      "GET",
      "GET",
    ]);
  }
});
