import { Toasty, useKumoToastManager } from "@cloudflare/kumo/components/toast";
import { useSetAtom } from "jotai";
import { useEffect, type ReactNode } from "react";
import { setToastSinkAtom } from "./toast-atoms";

function ToastSinkBridge({ children }: { children: ReactNode }) {
  const toastManager = useKumoToastManager();
  const setToastSink = useSetAtom(setToastSinkAtom);

  useEffect(() => {
    setToastSink((message, variant) =>
      toastManager.add({ title: message, variant }),
    );
    return () => setToastSink(null);
  }, [setToastSink, toastManager]);

  return children;
}

export function ToastBridge({ children }: { children: ReactNode }) {
  return (
    <Toasty>
      <ToastSinkBridge>{children}</ToastSinkBridge>
    </Toasty>
  );
}
