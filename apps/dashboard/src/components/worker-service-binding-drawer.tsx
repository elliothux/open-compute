import { Button } from "@cloudflare/kumo/components/button";
import { Input, Textarea } from "@cloudflare/kumo/components/input";
import { Select } from "@cloudflare/kumo/components/select";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { OpenComputeJsonValue } from "@open-compute/sdk";
import { useAuth } from "../features/auth/auth-atoms";
import { useMutationFeedback } from "../features/toast/use-mutation-feedback";
import { WorkerBindingDrawerLayout } from "./worker-binding-layout";
import {
  invalidateWorkerBindingQueries,
  saveWorkerResourceBinding,
} from "./worker-resource-binding-save";
import type { TwoColumnBindingDraft } from "./worker-two-column-binding-dialog";

function serviceProps(
  text: string,
): { readonly [key: string]: OpenComputeJsonValue } | null | undefined {
  if (!text.trim()) return undefined;
  try {
    const value: unknown = JSON.parse(text);
    if (
      value &&
      typeof value === "object" &&
      !Array.isArray(value) &&
      new TextEncoder().encode(JSON.stringify(value)).length <= 64 * 1024
    )
      return value as { readonly [key: string]: OpenComputeJsonValue };
  } catch {
    // Keep invalid JSON visible until corrected.
  }
  return null;
}

export function WorkerServiceBindingDrawer({
  workerId,
  bindingNames,
  draft,
  onChange,
  onClose,
}: {
  workerId: string;
  bindingNames: readonly string[];
  draft: TwoColumnBindingDraft | null;
  onChange: (draft: TwoColumnBindingDraft) => void;
  onClose: () => void;
}) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const resources = useQuery({
    queryKey: [
      "cloudflare-v4",
      "service",
      "binding-resources",
      selectedInstanceId,
    ],
    queryFn: async ({ signal }) => {
      const response = await client!.workers.scripts.list(
        { account_id: selectedInstanceId! },
        { signal },
      );
      return (response.result ?? []).flatMap((item) =>
        item.id ? [{ id: item.id, name: item.id }] : [],
      );
    },
    enabled: Boolean(client && selectedInstanceId && draft),
  });
  const rows = resources.data ?? [];
  const name = draft?.name.trim() ?? "";
  const entrypoint = draft?.entrypoint?.trim() ?? "";
  const props = serviceProps(draft?.propsText ?? "");
  const duplicate = bindingNames.some(
    (bindingName) =>
      bindingName === name && bindingName !== draft?.originalName,
  );
  const valid = Boolean(
    draft &&
    name &&
    draft.resourceId &&
    !duplicate &&
    rows.some((item) => item.id === draft.resourceId) &&
    props !== null &&
    (!entrypoint ||
      (entrypoint.length <= 128 &&
        /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(entrypoint))) &&
    (draft.originalName === null ||
      name !== draft.originalName ||
      draft.resourceId !== draft.originalResourceId ||
      entrypoint !== (draft.originalEntrypoint ?? "") ||
      JSON.stringify(props) !== draft.originalPropsText),
  );
  const save = useMutation({
    mutationFn: async (current: TwoColumnBindingDraft) => {
      if (!client || !selectedInstanceId)
        throw new Error("Dashboard session is unavailable.");
      const parsedProps = serviceProps(current.propsText ?? "");
      if (parsedProps === null)
        throw new Error(
          "Service props must be a JSON object no larger than 64 KiB.",
        );
      await saveWorkerResourceBinding(
        client,
        selectedInstanceId,
        workerId,
        bindingNames,
        {
          type: "service",
          originalName: current.originalName,
          name: current.name,
          service: current.resourceId,
          ...(current.entrypoint?.trim()
            ? { entrypoint: current.entrypoint.trim() }
            : {}),
          ...(parsedProps ? { props: parsedProps } : {}),
        },
      );
    },
    onSuccess: async () => {
      onClose();
      await invalidateWorkerBindingQueries(
        queryClient,
        selectedInstanceId!,
        workerId,
      );
      feedback.success("Service binding deployed.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to deploy Service binding."),
  });

  return (
    <WorkerBindingDrawerLayout
      open={draft !== null}
      pending={save.isPending}
      valid={valid}
      title="Service binding"
      description="Configure a binding to call another Worker service."
      docsHref="https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/"
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault();
        if (draft && valid) save.mutate(draft);
      }}
    >
      <Input
        label="Variable name"
        value={draft?.name ?? ""}
        onChange={(event) =>
          draft && onChange({ ...draft, name: event.target.value })
        }
        required
      />
      <Select
        label="Service binding"
        placeholder={
          resources.isPending ? "Loading services…" : "Select service"
        }
        value={draft?.resourceId ?? ""}
        items={rows.map((item) => ({
          label: item.name,
          value: item.id,
        }))}
        disabled={resources.isPending || Boolean(resources.error)}
        onValueChange={(value) =>
          draft && onChange({ ...draft, resourceId: value ?? "" })
        }
      />
      <div className="border-kumo-line grid gap-3 border-t pt-3">
        <Button
          className="w-fit"
          variant="ghost"
          size="sm"
          type="button"
          aria-expanded={advancedOpen}
          onClick={() => setAdvancedOpen((open) => !open)}
        >
          Advanced options
        </Button>
        {advancedOpen ? (
          <>
            <Input
              label="Named entrypoint"
              placeholder="Default entrypoint"
              value={draft?.entrypoint ?? ""}
              onChange={(event) =>
                draft &&
                onChange({
                  ...draft,
                  entrypoint: event.target.value,
                })
              }
            />
            <Textarea
              label="Props (JSON object)"
              className="min-h-24 font-mono text-xs"
              placeholder='{"tenant":"example"}'
              value={draft?.propsText ?? ""}
              onChange={(event) =>
                draft &&
                onChange({
                  ...draft,
                  propsText: event.target.value,
                })
              }
            />
          </>
        ) : null}
      </div>
      {props === null ? (
        <p className="text-kumo-danger">
          Enter a JSON object no larger than 64 KiB.
        </p>
      ) : null}
      {duplicate ? (
        <p className="text-kumo-danger">
          A binding with this name already exists.
        </p>
      ) : null}
      {resources.error ? (
        <p className="text-kumo-danger">Unable to load services.</p>
      ) : null}
      {save.isError ? (
        <p className="text-kumo-danger">{save.error.message}</p>
      ) : null}
    </WorkerBindingDrawerLayout>
  );
}
