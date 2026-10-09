import type { Inventory } from "./do/inventory";

interface InternalService extends Fetcher {
  echo(value: unknown): Promise<{ value: unknown; revision: string }>;
  multiply(a: number, b: number): Promise<number>;
}

export interface Env {
  REVISION: string;
  TOKEN: string;
  OUTBOUND_URL: string;
  KV: KVNamespace;
  DB: D1Database;
  BUCKET: R2Bucket;
  EVENTS: Queue;
  INVENTORY: DurableObjectNamespace<Inventory>;
  FLOW: Workflow;
  SERVICE: InternalService;
}
