import { atom } from "jotai";

export interface LiveLogRow extends Record<string, string> {
  id: string;
  timestamp: string;
  level: string;
  source: string;
}

export interface LiveTailState {
  workerId: string | null;
  enabled: boolean;
  status: "idle" | "connecting" | "live" | "error";
  error: string | null;
  rows: LiveLogRow[];
}

const emptyLiveTail = (workerId: string | null): LiveTailState => ({
  workerId,
  enabled: false,
  status: "idle",
  error: null,
  rows: [],
});

export const liveTailAtom = atom<LiveTailState>(emptyLiveTail(null));

export const selectLiveTailWorkerAtom = atom(
  null,
  (get, set, workerId: string) => {
    if (get(liveTailAtom).workerId !== workerId)
      set(liveTailAtom, emptyLiveTail(workerId));
  },
);

export const setLiveTailEnabledAtom = atom(
  null,
  (get, set, enabled: boolean) => {
    const current = get(liveTailAtom);
    set(liveTailAtom, {
      ...current,
      enabled,
      status: "idle",
      error: enabled ? null : current.error,
      rows: enabled ? [] : current.rows,
    });
  },
);

export const setLiveTailStatusAtom = atom(
  null,
  (get, set, status: LiveTailState["status"]) => {
    set(liveTailAtom, { ...get(liveTailAtom), status });
  },
);

export const failLiveTailAtom = atom(null, (get, set, error: string) => {
  set(liveTailAtom, {
    ...get(liveTailAtom),
    enabled: false,
    status: "error",
    error,
  });
});

export const clearLiveTailErrorAtom = atom(null, (get, set) => {
  set(liveTailAtom, { ...get(liveTailAtom), error: null });
});

export const prependLiveTailRowAtom = atom(
  null,
  (get, set, row: LiveLogRow) => {
    const current = get(liveTailAtom);
    set(liveTailAtom, {
      ...current,
      rows: [row, ...current.rows].slice(0, 100),
    });
  },
);
