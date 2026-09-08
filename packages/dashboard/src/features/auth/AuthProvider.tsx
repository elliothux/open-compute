import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { createManagementClient, type ManagementClient } from "../../lib/cloudflare";
import {
  clearAuthSession,
  exchangeLoginCode,
  readAuthSession,
  takeLoginCodeFromHash,
  writeAuthSession,
} from "./authSession";

interface AuthContextValue {
  token: string | null;
  accountId: string | null;
  ready: boolean;
  client: ManagementClient | null;
  setToken: (token: string | null) => void;
  setAccountId: (accountId: string | null) => void;
  clearAuth: () => void;
}

const AuthContext = createContext<AuthContextValue | null>(null);

function createClient(token: string | null): ManagementClient | null {
  if (!token) return null;
  return createManagementClient(token);
}

function initialAuthState(): { token: string | null; accountId: string | null } {
  const stored = readAuthSession();
  if (!stored) {
    return { token: null, accountId: null };
  }
  return stored;
}

async function resolveAccountId(token: string): Promise<string> {
  const client = createManagementClient(token);
  const accounts = await client.cloudflare.accounts.list();
  const account = accounts.result[0];
  if (account?.id === undefined) throw new Error("No accessible account was returned.");
  return account.id;
}

let capturedLoginCode: string | null | undefined;

function consumeLoginCodeOnce(): string | null {
  if (capturedLoginCode !== undefined) {
    return capturedLoginCode;
  }
  capturedLoginCode = takeLoginCodeFromHash();
  return capturedLoginCode;
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const [authState, setAuthState] = useState(initialAuthState);
  const [ready, setReady] = useState(() => consumeLoginCodeOnce() === null);

  useEffect(() => {
    let cancelled = false;
    async function bootstrap() {
      const code = consumeLoginCodeOnce();
      if (!code) {
        if (!cancelled) setReady(true);
        return;
      }
      try {
        const session = await exchangeLoginCode(code);
        const accountId = await resolveAccountId(session.session_token);
        if (cancelled) return;
        writeAuthSession(session.session_token, accountId);
        setAuthState({ token: session.session_token, accountId });
      } catch {
        if (!cancelled) {
          clearAuthSession();
          setAuthState({ token: null, accountId: null });
        }
      } finally {
        if (!cancelled) setReady(true);
      }
    }
    void bootstrap();
    return () => {
      cancelled = true;
    };
  }, []);

  const clearAuth = useCallback(() => {
    clearAuthSession();
    setAuthState({ token: null, accountId: null });
  }, []);

  const setToken = useCallback((next: string | null) => {
    if (!next) {
      clearAuthSession();
      setAuthState({ token: null, accountId: null });
      return;
    }
    setAuthState(previous => {
      if (previous.accountId) {
        writeAuthSession(next, previous.accountId);
      }
      return { ...previous, token: next };
    });
  }, []);

  const setAccountId = useCallback((next: string | null) => {
    setAuthState(previous => {
      if (!next) {
        if (!previous.token) clearAuthSession();
        return { ...previous, accountId: null };
      }
      if (previous.token) {
        writeAuthSession(previous.token, next);
      }
      return { ...previous, accountId: next };
    });
  }, []);

  const value = useMemo<AuthContextValue>(() => ({
    token: authState.token,
    accountId: authState.accountId,
    ready,
    client: createClient(authState.token),
    setToken,
    setAccountId,
    clearAuth,
  }), [authState.token, authState.accountId, ready, setToken, setAccountId, clearAuth]);

  if (!ready) {
    return null;
  }

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth() {
  const context = useContext(AuthContext);
  if (!context) throw new Error("useAuth must be used within AuthProvider");
  return context;
}
