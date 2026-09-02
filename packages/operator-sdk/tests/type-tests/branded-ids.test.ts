import {
  createOperatorClient,
  parseAccountId,
  parseDeploymentId,
  parseWorkerId,
  type AccountId,
  type OperatorClient,
} from "../../src/index.js";

const baseUrl = new URL("http://127.0.0.1:8788/operator/api/v1/");
const client = createOperatorClient({
  baseUrl,
  getAccessToken: () => "admin-secret",
});

const accountId: AccountId = parseAccountId("01900000-0000-7000-8000-000000000001");
const workerId = parseWorkerId("01900000-0000-7000-8000-000000000002");
const deploymentId = parseDeploymentId("01900000-0000-7000-8000-000000000003");

void client.workers.list({ accountId });
void client.workers.promote({
  accountId,
  workerId,
  targetDeploymentId: deploymentId,
  idempotencyKey: "promote-1",
});

// @ts-expect-error promote requires an idempotency key
void client.workers.promote({
  accountId,
  workerId,
  targetDeploymentId: deploymentId,
});

// @ts-expect-error accountId must be parsed or branded
void client.workers.list({ accountId: "01900000-0000-7000-8000-000000000001" });

type PromoteParams = Parameters<OperatorClient["workers"]["promote"]>[0];
const badPromote: PromoteParams = {
  accountId,
  workerId,
  // @ts-expect-error deployment id must be branded
  targetDeploymentId: "01900000-0000-7000-8000-000000000003",
  idempotencyKey: "promote-2",
};
void badPromote;
