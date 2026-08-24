"use strict";

const fs = require("node:fs");
const path = require("node:path");

const FIXTURES = path.resolve(__dirname, "..", "fixtures");

const KEYS = {
  A: "acct_fixture/worker_fixture/deploy_a",
  B: "acct_fixture/worker_fixture/deploy_b",
  BAD: "acct_fixture/worker_fixture/deploy_bad_syntax",
  MISSING: "acct_fixture/worker_fixture/deploy_missing_module",
  THROW: "acct_fixture/worker_fixture/deploy_throw_startup",
  KV_A: "acct_fixture/worker_kv/deploy_a",
  KV_B: "acct_fixture/worker_kv/deploy_b",
  DO_A: "acct_fixture/worker_do/deploy_a",
  DO_B: "acct_fixture/worker_do/deploy_b",
  OUT: "acct_fixture/worker_out/deploy_a",
};

function readFixture(relPath) {
  return fs.readFileSync(path.join(FIXTURES, relPath), "utf8");
}

function listFixtureFiles(dir) {
  return fs.readdirSync(path.join(FIXTURES, dir)).sort();
}

module.exports = {
  FIXTURES,
  KEYS,
  readFixture,
  listFixtureFiles,
};
