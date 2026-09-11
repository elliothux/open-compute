import { atom, useSetAtom } from "jotai";

type ToastVariant = "success" | "error" | "info";
export type ToastSink = (message: string, variant: ToastVariant) => void;

const toastSinkAtom = atom<ToastSink | null>(null);

export const setToastSinkAtom = atom(
  null,
  (_get, set, sink: ToastSink | null) => {
    // jotai treats a function value as an updater — wrap so the sink itself is stored
    set(toastSinkAtom, () => sink);
  },
);

const pushToastAtom = atom(
  null,
  (get, _set, message: string, variant: ToastVariant = "info") => {
    get(toastSinkAtom)?.(message, variant);
  },
);

export function useToast() {
  const pushToast = useSetAtom(pushToastAtom);
  return { pushToast };
}
