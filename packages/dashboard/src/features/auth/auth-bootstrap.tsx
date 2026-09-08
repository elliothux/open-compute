import { useAtomValue, useSetAtom } from "jotai";
import { useEffect, type ReactNode } from "react";
import { authReadyAtom, bootstrapAuthAtom } from "./auth-atoms";

export function AuthBootstrap({ children }: { children: ReactNode }) {
  const ready = useAtomValue(authReadyAtom);
  const bootstrap = useSetAtom(bootstrapAuthAtom);

  useEffect(() => {
    void bootstrap();
  }, [bootstrap]);

  return ready ? children : null;
}
