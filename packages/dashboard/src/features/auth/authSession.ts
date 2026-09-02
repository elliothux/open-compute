const AUTH_SESSION_KEY = "open-compute.operator.auth";

interface StoredAuthSession {
  token: string;
  accountId: string;
}

function isStoredAuthSession(value: unknown): value is StoredAuthSession {
  if (value === null || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  return typeof record.token === "string"
    && record.token.length > 0
    && typeof record.accountId === "string"
    && record.accountId.length > 0;
}

/** Read the persisted operator session for this browser tab. */
export function readAuthSession(): { token: string; accountId: string } | null {
  try {
    const raw = sessionStorage.getItem(AUTH_SESSION_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (!isStoredAuthSession(parsed)) {
      sessionStorage.removeItem(AUTH_SESSION_KEY);
      return null;
    }
    return {
      token: parsed.token,
      accountId: parsed.accountId,
    };
  } catch {
    sessionStorage.removeItem(AUTH_SESSION_KEY);
    return null;
  }
}

/** Persist operator credentials for refresh recovery within the same tab. */
export function writeAuthSession(token: string, accountId: string): void {
  const payload: StoredAuthSession = { token, accountId };
  sessionStorage.setItem(AUTH_SESSION_KEY, JSON.stringify(payload));
}

/** Drop any persisted operator session for this tab. */
export function clearAuthSession(): void {
  sessionStorage.removeItem(AUTH_SESSION_KEY);
}
