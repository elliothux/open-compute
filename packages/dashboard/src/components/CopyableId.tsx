import { useState } from "react";
import { Check, Copy, Warning } from "@phosphor-icons/react";
import { Button } from "@cloudflare/kumo/components/button";

export function CopyableId({ value, label = "resource ID" }: { value: string; label?: string }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setState("copied");
    } catch {
      setState("failed");
    }
  };

  const status = state === "copied" ? "Copied" : state === "failed" ? "Copy failed" : `Copy ${label}`;
  const icon = state === "copied" ? Check : state === "failed" ? Warning : Copy;

  return (
    <span className="inline-flex min-w-0 items-center gap-1.5">
      <code className="max-w-full overflow-hidden text-ellipsis [font-size:0.9em]">{value}</code>
      <Button
        variant="ghost"
        size="sm"
        shape="square"
        icon={icon}
        aria-label={status}
        title={status}
        onClick={() => void copy()}
      />
      <span className="sr-only" aria-live="polite">{state === "idle" ? "" : status}</span>
    </span>
  );
}
