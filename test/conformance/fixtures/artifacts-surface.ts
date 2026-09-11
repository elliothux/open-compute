declare const artifacts: Artifacts;

async function exerciseArtifacts(): Promise<void> {
  const created = await artifacts.create("site", {
    description: "site source",
    readOnly: false,
    setDefaultBranch: "main",
  });
  const creationFields: [
    string,
    string,
    string | null,
    string,
    string,
    string,
    string,
  ] = [
    created.id,
    created.name,
    created.description,
    created.defaultBranch,
    created.remote,
    created.token,
    created.tokenExpiresAt,
  ];

  const repo = await artifacts.get(created.name);
  const repositoryFields: [
    string,
    string,
    string | null,
    string,
    string,
    string,
    string | null,
    string | null,
    boolean,
    string,
  ] = [
    repo.id,
    repo.name,
    repo.description,
    repo.defaultBranch,
    repo.createdAt,
    repo.updatedAt,
    repo.lastPushAt,
    repo.source,
    repo.readOnly,
    repo.remote,
  ];
  const token = await repo.createToken("read", 60);
  const tokenFields: [string, string, "read" | "write", string] = [
    token.id,
    token.plaintext,
    token.scope,
    token.expiresAt,
  ];
  const listedTokens = await repo.listTokens();
  const tokenListFields: [
    number,
    string,
    "read" | "write",
    "active" | "expired" | "revoked",
    string,
    string,
  ] = [
    listedTokens.total,
    listedTokens.tokens[0]!.id,
    listedTokens.tokens[0]!.scope,
    listedTokens.tokens[0]!.state,
    listedTokens.tokens[0]!.createdAt,
    listedTokens.tokens[0]!.expiresAt,
  ];
  await repo.revokeToken(token.id);
  await repo.fork("site-fork", { defaultBranchOnly: true });
  await artifacts.import({
    source: { url: "https://example.com/site.git", branch: "main", depth: 1 },
    target: {
      name: "site-import",
      opts: { description: "imported", readOnly: true },
    },
  });
  const listed = await artifacts.list({ limit: 50, cursor: "next" });
  const listFields: [
    number,
    string | undefined,
    Omit<ArtifactsRepoInfo, "remote">[],
  ] = [listed.total, listed.cursor, listed.repos];
  await artifacts.delete("site");
  void [
    creationFields,
    repositoryFields,
    tokenFields,
    tokenListFields,
    listFields,
  ];
}

function inspectArtifactsError(
  error: ArtifactsError,
): ["ArtifactsError", ArtifactsErrorCode, number] {
  return [error.name, error.code, error.numericCode];
}

void exerciseArtifacts;
void inspectArtifactsError;
