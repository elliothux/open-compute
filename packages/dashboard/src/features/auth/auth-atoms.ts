import { atom, useAtomValue, useSetAtom } from "jotai";
import { useMemo } from "react";
import {
  createManagementClient,
  type ManagementClient,
} from "../../lib/cloudflare";
import {
  clearAuthSession,
  exchangeLoginCode,
  readAuthSession,
  takeLoginCodeFromHash,
  writeAuthSession,
} from "./auth-session";

interface AuthState {
  token: string | null;
  accountId: string | null;
}

const browser = typeof window === "undefined" ? null : window;
const loginCode = browser === null ? null : takeLoginCodeFromHash();
const storedSession = browser === null ? null : readAuthSession();
const authStateAtom = atom<AuthState>(
  storedSession ?? { token: null, accountId: null },
);
export const authReadyAtom = atom(loginCode === null);
export const authTokenAtom = atom((get) => get(authStateAtom).token);
export const authAccountIdAtom = atom((get) => get(authStateAtom).accountId);
export const authClientAtom = atom<ManagementClient | null>((get) => {
  const token = get(authTokenAtom);
  return token === null ? null : createManagementClient(token);
});

export const clearAuthAtom = atom(null, (_get, set) => {
  clearAuthSession();
  set(authStateAtom, { token: null, accountId: null });
});

export const setAuthTokenAtom = atom(null, (get, set, token: string | null) => {
  if (token === null) {
    clearAuthSession();
    set(authStateAtom, { token: null, accountId: null });
    return;
  }
  const previous = get(authStateAtom);
  if (previous.accountId !== null) writeAuthSession(token, previous.accountId);
  set(authStateAtom, { ...previous, token });
});

export const setAuthAccountIdAtom = atom(
  null,
  (get, set, accountId: string | null) => {
    const previous = get(authStateAtom);
    if (accountId === null) {
      if (previous.token === null) clearAuthSession();
      set(authStateAtom, { ...previous, accountId: null });
      return;
    }
    if (previous.token !== null) writeAuthSession(previous.token, accountId);
    set(authStateAtom, { ...previous, accountId });
  },
);

async function resolveAccountId(token: string): Promise<string> {
  const accounts =
    await createManagementClient(token).cloudflare.accounts.list();
  const account = accounts.result[0];
  if (account?.id === undefined)
    throw new Error("No accessible account was returned.");
  return account.id;
}

export const bootstrapAuthAtom = atom(null, async (_get, set) => {
  if (loginCode === null) {
    set(authReadyAtom, true);
    return;
  }
  try {
    const session = await exchangeLoginCode(loginCode);
    const accountId = await resolveAccountId(session.session_token);
    writeAuthSession(session.session_token, accountId);
    set(authStateAtom, { token: session.session_token, accountId });
  } catch {
    clearAuthSession();
    set(authStateAtom, { token: null, accountId: null });
  } finally {
    set(authReadyAtom, true);
  }
});

export function useAuth() {
  const token = useAtomValue(authTokenAtom);
  const accountId = useAtomValue(authAccountIdAtom);
  const client = useAtomValue(authClientAtom);
  const ready = useAtomValue(authReadyAtom);
  const setToken = useSetAtom(setAuthTokenAtom);
  const setAccountId = useSetAtom(setAuthAccountIdAtom);
  const clearAuth = useSetAtom(clearAuthAtom);
  return useMemo(
    () => ({
      token,
      accountId,
      client,
      ready,
      setToken,
      setAccountId,
      clearAuth,
    }),
    [token, accountId, client, ready, setToken, setAccountId, clearAuth],
  );
}
