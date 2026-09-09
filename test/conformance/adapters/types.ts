export type JsonRecord = Record<string, unknown>;

export interface CommandResult {
  readonly status: number;
  readonly stdout: string;
  readonly stderr: string;
}

export interface Observation {
  readonly method: string;
  readonly path: string;
  readonly headers: Readonly<Record<string, string>>;
  readonly body?: Uint8Array;
  readonly expect: { readonly status: number; readonly json: unknown };
}

export interface PortableFixture {
  readonly id: string;
  readonly root: string;
  readonly source: string;
  readonly sourceSha256: string;
  readonly contracts: readonly string[];
  readonly bindings: Readonly<Record<string, PortableBinding>>;
  readonly observations: readonly Observation[];
}

export type PortableBinding =
  | {
      readonly type:
        | "kv_namespace"
        | "d1_database"
        | "r2_bucket"
        | "queue_producer"
        | "worker_loader";
    }
  | {
      readonly type: "do_namespace" | "workflow";
      readonly className: string;
      readonly schedules?: readonly string[];
    };
