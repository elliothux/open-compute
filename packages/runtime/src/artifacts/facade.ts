interface ArtifactsRawTransport {
  call(operation: string, input: unknown): Promise<unknown>;
}

const ERROR_CODES = new Set<ArtifactsErrorCode>([
  "ALREADY_EXISTS",
  "NOT_FOUND",
  "IMPORT_IN_PROGRESS",
  "FORK_IN_PROGRESS",
  "INVALID_INPUT",
  "INVALID_REPO_NAME",
  "INVALID_TTL",
  "INVALID_URL",
  "REMOTE_AUTH_REQUIRED",
  "UPSTREAM_UNAVAILABLE",
  "MEMORY_LIMIT",
  "INTERNAL_ERROR",
]);

export class ArtifactError extends Error implements ArtifactsError {
  readonly name = "ArtifactsError";
  constructor(
    readonly code: ArtifactsErrorCode,
    readonly numericCode: number,
  ) {
    super(code);
  }
}

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function protocolError(): never {
  throw new ArtifactError("INTERNAL_ERROR", 10400);
}

function transport(value: unknown): ArtifactsRawTransport {
  if (!record(value) || typeof value.call !== "function") protocolError();
  const invoke = value.call;
  return {
    call(operation, input) {
      return Promise.resolve(Reflect.apply(invoke, value, [operation, input]));
    },
  };
}

function error(value: unknown): ArtifactError {
  if (record(value)) {
    const code = value.code;
    const numericCode = value.numericCode;
    if (
      typeof code === "string" &&
      ERROR_CODES.has(code as ArtifactsErrorCode) &&
      Number.isSafeInteger(numericCode)
    ) {
      return new ArtifactError(
        code as ArtifactsErrorCode,
        numericCode as number,
      );
    }
  }
  return new ArtifactError("INTERNAL_ERROR", 10400);
}

async function call(
  raw: ArtifactsRawTransport,
  operation: string,
  input: unknown,
): Promise<unknown> {
  try {
    return await raw.call(operation, input);
  } catch (cause) {
    throw error(cause);
  }
}

function string(value: unknown): string {
  if (typeof value !== "string") protocolError();
  return value;
}

function nullableString(value: unknown): string | null {
  if (value !== null && typeof value !== "string") protocolError();
  return value;
}

function repoInfo(value: unknown): ArtifactsRepoInfo {
  if (!record(value)) protocolError();
  return {
    id: string(value.id),
    name: string(value.name),
    description: nullableString(value.description),
    defaultBranch: string(value.defaultBranch),
    createdAt: string(value.createdAt),
    updatedAt: string(value.updatedAt),
    lastPushAt: nullableString(value.lastPushAt),
    source: nullableString(value.source),
    readOnly:
      typeof value.readOnly === "boolean" ? value.readOnly : protocolError(),
    remote: string(value.remote),
  };
}

function created(value: unknown): ArtifactsCreateRepoResult {
  if (!record(value)) protocolError();
  return {
    id: string(value.id),
    name: string(value.name),
    description: nullableString(value.description),
    defaultBranch: string(value.defaultBranch),
    remote: string(value.remote),
    token: string(value.token),
    tokenExpiresAt: string(value.tokenExpiresAt),
  };
}

function createdToken(value: unknown): ArtifactsCreateTokenResult {
  if (!record(value)) protocolError();
  const scope = value.scope;
  if (scope !== "read" && scope !== "write") protocolError();
  return {
    id: string(value.id),
    plaintext: string(value.plaintext),
    scope,
    expiresAt: string(value.expiresAt),
  };
}

function listedTokens(value: unknown): ArtifactsTokenListResult {
  if (!record(value) || !Array.isArray(value.tokens)) protocolError();
  const tokens = value.tokens.map((raw): ArtifactsTokenInfo => {
    if (!record(raw)) protocolError();
    const scope = raw.scope;
    const state = raw.state;
    if (
      (scope !== "read" && scope !== "write") ||
      (state !== "active" && state !== "expired" && state !== "revoked")
    )
      protocolError();
    return {
      id: string(raw.id),
      scope,
      state,
      createdAt: string(raw.createdAt),
      expiresAt: string(raw.expiresAt),
    };
  });
  if (!Number.isSafeInteger(value.total) || value.total !== tokens.length)
    protocolError();
  return { tokens, total: value.total as number };
}

class ArtifactRepoHandle implements ArtifactsRepo {
  readonly id: string;
  readonly name: string;
  readonly description: string | null;
  readonly defaultBranch: string;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly lastPushAt: string | null;
  readonly source: string | null;
  readonly readOnly: boolean;
  readonly remote: string;

  constructor(
    private readonly raw: ArtifactsRawTransport,
    info: ArtifactsRepoInfo,
  ) {
    this.id = info.id;
    this.name = info.name;
    this.description = info.description;
    this.defaultBranch = info.defaultBranch;
    this.createdAt = info.createdAt;
    this.updatedAt = info.updatedAt;
    this.lastPushAt = info.lastPushAt;
    this.source = info.source;
    this.readOnly = info.readOnly;
    this.remote = info.remote;
  }

  async createToken(scope?: "write" | "read", ttl?: number) {
    return createdToken(
      await call(this.raw, "create-token", {
        repository: this.name,
        scope,
        ttl,
      }),
    );
  }

  async listTokens() {
    return listedTokens(
      await call(this.raw, "list-tokens", { repository: this.name }),
    );
  }

  async revokeToken(tokenOrId: string) {
    const value = await call(this.raw, "revoke-token", {
      repository: this.name,
      tokenOrId,
    });
    return typeof value === "boolean" ? value : protocolError();
  }

  async fork(
    name: string,
    opts?: {
      description?: string;
      readOnly?: boolean;
      defaultBranchOnly?: boolean;
    },
  ) {
    return created(
      await call(this.raw, "fork", {
        repository: this.name,
        name,
        opts,
      }),
    );
  }
}

/** Latest pinned Cloudflare Artifacts API over a private namespace transport. */
export class ArtifactsBinding implements Artifacts {
  readonly #raw: ArtifactsRawTransport;
  constructor(raw: unknown) {
    this.#raw = transport(raw);
  }

  async create(
    name: string,
    opts?: {
      readOnly?: boolean;
      description?: string;
      setDefaultBranch?: string;
    },
  ) {
    return created(await call(this.#raw, "create", { name, opts }));
  }

  async get(name: string): Promise<ArtifactsRepo> {
    return new ArtifactRepoHandle(
      this.#raw,
      repoInfo(await call(this.#raw, "get", { name })),
    );
  }

  async import(params: {
    source: { url: string; branch?: string; depth?: number };
    target: {
      name: string;
      opts?: { description?: string; readOnly?: boolean };
    };
  }) {
    return created(await call(this.#raw, "import", params));
  }

  async list(opts?: { limit?: number; cursor?: string }) {
    const value = await call(this.#raw, "list", opts ?? {});
    if (!record(value) || !Array.isArray(value.repos)) protocolError();
    const repos = value.repos.map((raw) => {
      if (!record(raw)) protocolError();
      const info = repoInfo({ ...raw, remote: "" });
      return {
        id: info.id,
        name: info.name,
        description: info.description,
        defaultBranch: info.defaultBranch,
        createdAt: info.createdAt,
        updatedAt: info.updatedAt,
        lastPushAt: info.lastPushAt,
        source: info.source,
        readOnly: info.readOnly,
      };
    });
    if (!Number.isSafeInteger(value.total)) protocolError();
    const result: ArtifactsRepoListResult = {
      repos,
      total: value.total as number,
    };
    if (value.cursor !== undefined) result.cursor = string(value.cursor);
    return result;
  }

  async delete(name: string) {
    const value = await call(this.#raw, "delete", { name });
    return typeof value === "boolean" ? value : protocolError();
  }
}
