import {
  createOperatorClient,
  parseAccountId,
  parseResourceId,
  type AccountId,
  type OperatorClient,
} from "../../src/index.js";

const client = createOperatorClient({
  baseUrl: new URL("http://127.0.0.1:8788/operator/api/v1/"),
  getAccessToken: () => "admin-secret",
});

const accountId: AccountId = parseAccountId("01900000-0000-7000-8000-000000000001");
const namespaceId = parseResourceId("01900000-0000-7000-8000-000000000010");

// @ts-expect-error idempotent KV writes require an idempotency key
void client.kv.putValue({
  accountId,
  namespaceId,
  key: "hello",
  value: "world",
});

void client.kv.putValue({
  accountId,
  // @ts-expect-error namespaceId must be branded
  namespaceId: "01900000-0000-7000-8000-000000000010",
  key: "hello",
  value: "world",
  idempotencyKey: "idem",
});

type ListRoutesParams = Parameters<OperatorClient["workers"]["listRoutes"]>[0];
const badRoutes: ListRoutesParams = {
  accountId,
  // @ts-expect-error workerId must be branded
  workerId: "01900000-0000-7000-8000-000000000002",
};
void badRoutes;
