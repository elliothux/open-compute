import { atom, useAtomValue, useSetAtom } from "jotai";
import { useMemo } from "react";
import {
  createManagementClient,
  type ManagementClient,
} from "../../lib/management-client";
import {
  clearAuthSession,
  exchangeLoginCode,
  readAuthSession,
  revokeSession,
  takeLoginCodeFromHash,
  writeAuthSession,
} from "./auth-session";

interface AuthState {
  token: string | null;
  instanceId: string | null;
}

const browser = typeof window === "undefined" ? null : window;
const loginCode = browser === null ? null : takeLoginCodeFromHash();
const storedSession = browser === null ? null : readAuthSession();
const authStateAtom = atom<AuthState>(
  storedSession ?? { token: null, instanceId: null },
);
export const authReadyAtom = atom(loginCode === null);
export const authTokenAtom = atom((get) => get(authStateAtom).token);
export const authInstanceIdAtom = atom((get) => get(authStateAtom).instanceId);
export const authClientAtom = atom<ManagementClient | null>((get) => {
  const token = get(authTokenAtom);
  return token === null ? null : createManagementClient(token);
});

export const clearAuthAtom = atom(null, (_get, set) => {
  clearAuthSession();
  set(authStateAtom, { token: null, instanceId: null });
});

/** Revoke the server session best-effort, then drop all local auth state. */
export const signOutAtom = atom(null, async (get, set) => {
  const token = get(authTokenAtom);
  if (token !== null) await revokeSession(token).catch(() => undefined);
  set(clearAuthAtom);
});

export const setAuthTokenAtom = atom(null, (get, set, token: string | null) => {
  if (token === null) {
    clearAuthSession();
    set(authStateAtom, { token: null, instanceId: null });
    return;
  }
  const previous = get(authStateAtom);
  if (previous.instanceId !== null)
    writeAuthSession(token, previous.instanceId);
  set(authStateAtom, { ...previous, token });
});

export const setAuthInstanceIdAtom = atom(
  null,
  (get, set, instanceId: string | null) => {
    const previous = get(authStateAtom);
    if (instanceId === null) {
      if (previous.token === null) clearAuthSession();
      set(authStateAtom, { ...previous, instanceId: null });
      return;
    }
    if (previous.token !== null) writeAuthSession(previous.token, instanceId);
    set(authStateAtom, { ...previous, instanceId });
  },
);

async function resolveInstanceId(token: string): Promise<string> {
  const accounts = await createManagementClient(token).accounts.list();
  const instance = accounts.result[0];
  if (instance?.id === undefined)
    throw new Error("No accessible instance was returned.");
  return instance.id;
}

export const bootstrapAuthAtom = atom(null, async (_get, set) => {
  if (loginCode === null) {
    set(authReadyAtom, true);
    return;
  }
  try {
    const session = await exchangeLoginCode(loginCode);
    const instanceId = await resolveInstanceId(session.session_token);
    writeAuthSession(session.session_token, instanceId);
    set(authStateAtom, { token: session.session_token, instanceId });
  } catch {
    clearAuthSession();
    set(authStateAtom, { token: null, instanceId: null });
  } finally {
    set(authReadyAtom, true);
  }
});

export function useAuth() {
  const token = useAtomValue(authTokenAtom);
  const instanceId = useAtomValue(authInstanceIdAtom);
  const client = useAtomValue(authClientAtom);
  const ready = useAtomValue(authReadyAtom);
  const setToken = useSetAtom(setAuthTokenAtom);
  const setInstanceId = useSetAtom(setAuthInstanceIdAtom);
  const clearAuth = useSetAtom(clearAuthAtom);
  const signOut = useSetAtom(signOutAtom);
  return useMemo(
    () => ({
      token,
      instanceId,
      client,
      ready,
      setToken,
      setInstanceId,
      clearAuth,
      signOut,
    }),
    [
      token,
      instanceId,
      client,
      ready,
      setToken,
      setInstanceId,
      clearAuth,
      signOut,
    ],
  );
}
