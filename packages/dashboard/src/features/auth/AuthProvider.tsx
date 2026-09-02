import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";
import { createManagementClient, type ManagementClient } from "../../lib/cloudflare";
import { clearAuthSession, readAuthSession, writeAuthSession } from "./authSession";

interface AuthContextValue {
  token: string | null;
  accountId: string | null;
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

export function AuthProvider({ children }: { children: ReactNode }) {
  const [authState, setAuthState] = useState(initialAuthState);

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
    client: createClient(authState.token),
    setToken,
    setAccountId,
    clearAuth,
  }), [authState.token, authState.accountId, setToken, setAccountId, clearAuth]);

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth() {
  const context = useContext(AuthContext);
  if (!context) throw new Error("useAuth must be used within AuthProvider");
  return context;
}
