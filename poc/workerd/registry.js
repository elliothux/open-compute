export const COMPAT_DATE = "2026-08-22";
export const ACCOUNT_ID = "acct_fixture";
export const WORKER_ID = "worker_fixture";
export const KV_WORKER_ID = "worker_kv";
export const DO_WORKER_ID = "worker_do";
export const OUT_WORKER_ID = "worker_out";

export const DEPLOYMENTS = {
  [`${ACCOUNT_ID}/${WORKER_ID}/deploy_a`]: {
    accountId: ACCOUNT_ID,
    workerId: WORKER_ID,
    deploymentId: "deploy_a",
    root: "worker-a",
    mainModule: "index.js",
    files: ["index.js", "dep.js"],
    kind: "fetch",
  },
  [`${ACCOUNT_ID}/${WORKER_ID}/deploy_b`]: {
    accountId: ACCOUNT_ID,
    workerId: WORKER_ID,
    deploymentId: "deploy_b",
    root: "worker-b",
    mainModule: "index.js",
    files: ["index.js", "dep.js"],
    kind: "fetch",
  },
  [`${ACCOUNT_ID}/${WORKER_ID}/deploy_bad_syntax`]: {
    accountId: ACCOUNT_ID,
    workerId: WORKER_ID,
    deploymentId: "deploy_bad_syntax",
    root: "bad-syntax",
    mainModule: "index.js",
    files: ["index.js"],
    kind: "fetch",
  },
  [`${ACCOUNT_ID}/${WORKER_ID}/deploy_missing_module`]: {
    accountId: ACCOUNT_ID,
    workerId: WORKER_ID,
    deploymentId: "deploy_missing_module",
    root: "missing-module",
    mainModule: "index.js",
    files: ["index.js"],
    kind: "fetch",
  },
  [`${ACCOUNT_ID}/${WORKER_ID}/deploy_throw_startup`]: {
    accountId: ACCOUNT_ID,
    workerId: WORKER_ID,
    deploymentId: "deploy_throw_startup",
    root: "throw-startup",
    mainModule: "index.js",
    files: ["index.js"],
    kind: "fetch",
  },
  [`${ACCOUNT_ID}/${KV_WORKER_ID}/deploy_a`]: {
    accountId: ACCOUNT_ID,
    workerId: KV_WORKER_ID,
    deploymentId: "deploy_a",
    root: "binding-client",
    mainModule: "index.js",
    files: ["index.js"],
    kind: "binding",
    resourceId: "kv_fixture_a",
  },
  [`${ACCOUNT_ID}/${KV_WORKER_ID}/deploy_b`]: {
    accountId: ACCOUNT_ID,
    workerId: KV_WORKER_ID,
    deploymentId: "deploy_b",
    root: "binding-client",
    mainModule: "index.js",
    files: ["index.js"],
    kind: "binding",
    resourceId: "kv_fixture_b",
  },
  [`${ACCOUNT_ID}/${DO_WORKER_ID}/deploy_a`]: {
    accountId: ACCOUNT_ID,
    workerId: DO_WORKER_ID,
    deploymentId: "deploy_a",
    root: "do-counter",
    mainModule: "a.js",
    files: ["a.js"],
    kind: "do",
    className: "Counter",
  },
  [`${ACCOUNT_ID}/${DO_WORKER_ID}/deploy_b`]: {
    accountId: ACCOUNT_ID,
    workerId: DO_WORKER_ID,
    deploymentId: "deploy_b",
    root: "do-counter",
    mainModule: "b.js",
    files: ["b.js"],
    kind: "do",
    className: "Counter",
  },
  [`${ACCOUNT_ID}/${OUT_WORKER_ID}/deploy_a`]: {
    accountId: ACCOUNT_ID,
    workerId: OUT_WORKER_ID,
    deploymentId: "deploy_a",
    root: "outbound",
    mainModule: "index.js",
    files: ["index.js"],
    kind: "fetch",
  },
};

export function loaderKey(accountId, workerId, deploymentId) {
  return `${accountId}/${workerId}/${deploymentId}`;
}

export function getDeployment(key) {
  return DEPLOYMENTS[key] || null;
}
