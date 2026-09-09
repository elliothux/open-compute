const AUTH_SESSION_KEY = "open-compute.operator.auth";

interface StoredAuthSession {
  token: string;
  accountId: string;
}

function isStoredAuthSession(value: unknown): value is StoredAuthSession {
  if (value === null || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  return (
    typeof record.token === "string" &&
    record.token.length > 0 &&
    typeof record.accountId === "string" &&
    record.accountId.length > 0
  );
}

/** Read the short-lived operator session for this browser tab. */
export function readAuthSession(): { token: string; accountId: string } | null {
  if (typeof sessionStorage === "undefined") return null;
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

/** Persist a short-lived browser session for refresh recovery within the same tab. */
export function writeAuthSession(token: string, accountId: string): void {
  if (typeof sessionStorage === "undefined") return;
  const payload: StoredAuthSession = { token, accountId };
  sessionStorage.setItem(AUTH_SESSION_KEY, JSON.stringify(payload));
}

/** Drop any persisted operator session for this tab. */
export function clearAuthSession(): void {
  if (typeof sessionStorage === "undefined") return;
  sessionStorage.removeItem(AUTH_SESSION_KEY);
}

/** Take `#login=<code>` from the URL hash and clear it immediately. */
export function takeLoginCodeFromHash(): string | null {
  if (typeof window === "undefined") return null;
  const hash = window.location.hash.startsWith("#")
    ? window.location.hash.slice(1)
    : window.location.hash;
  if (!hash.startsWith("login=")) return null;
  const code = decodeURIComponent(hash.slice("login=".length)).trim();
  const path = `${window.location.pathname}${window.location.search}`;
  window.history.replaceState(null, "", path);
  return code.length > 0 ? code : null;
}

interface SessionIssueResponse {
  session_token: string;
  expires_at_ms: number;
}

function isSessionIssueResponse(value: unknown): value is SessionIssueResponse {
  if (value === null || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  return (
    typeof record.session_token === "string" &&
    record.session_token.length > 0 &&
    typeof record.expires_at_ms === "number"
  );
}

/** Exchange a one-time login code for a short browser session. */
export async function exchangeLoginCode(
  code: string,
): Promise<SessionIssueResponse> {
  const response = await fetch(
    new URL("/operator/session/exchange", window.location.origin),
    {
      method: "POST",
      credentials: "same-origin",
      headers: {
        "content-type": "application/json",
      },
      body: JSON.stringify({ code }),
    },
  );
  if (!response.ok) {
    throw new Error("login code exchange failed");
  }
  const payload: unknown = await response.json();
  if (!isSessionIssueResponse(payload)) {
    throw new Error("login code exchange returned an invalid session");
  }
  return payload;
}

/** Mint a short browser session from a long-lived admin token. */
export async function mintSessionFromAdmin(
  adminToken: string,
): Promise<SessionIssueResponse> {
  const response = await fetch(
    new URL("/operator/session", window.location.origin),
    {
      method: "POST",
      credentials: "same-origin",
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${adminToken}`,
      },
      body: "{}",
    },
  );
  if (!response.ok) {
    throw new Error("admin session mint failed");
  }
  const payload: unknown = await response.json();
  if (!isSessionIssueResponse(payload)) {
    throw new Error("admin session mint returned an invalid session");
  }
  return payload;
}
