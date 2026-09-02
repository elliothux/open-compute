import { createContext, useCallback, useContext, useMemo, type ReactNode } from "react";
import { Toasty, useKumoToastManager } from "@cloudflare/kumo/components/toast";

export type ToastVariant = "success" | "error" | "info";

interface ToastContextValue {
  pushToast: (message: string, variant?: ToastVariant) => void;
}

const ToastContext = createContext<ToastContextValue | null>(null);

export function ToastProvider({ children }: { children: ReactNode }) {
  return (
    <Toasty>
      <ToastBridge>{children}</ToastBridge>
    </Toasty>
  );
}

function ToastBridge({ children }: { children: ReactNode }) {
  const toastManager = useKumoToastManager();

  const pushToast = useCallback((message: string, variant: ToastVariant = "info") => {
    toastManager.add({ title: message, variant });
  }, [toastManager]);

  const value = useMemo(() => ({ pushToast }), [pushToast]);

  return (
    <ToastContext.Provider value={value}>
      {children}
    </ToastContext.Provider>
  );
}

export function useToast() {
  const context = useContext(ToastContext);
  if (!context) {
    throw new Error("useToast must be used within ToastProvider");
  }
  return context;
}
