import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";
import { Select } from "@cloudflare/kumo/components/select";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAuth } from "../features/auth/auth-atoms";
import { useMutationFeedback } from "../features/toast/use-mutation-feedback";
import { CodeBlock } from "./code-block";
import { WorkerBindingDialogLayout } from "./worker-binding-layout";
import {
  invalidateWorkerBindingQueries,
  saveWorkerResourceBinding,
  type BindingKind,
} from "./worker-resource-binding-save";

export type AiSearchBindingDraft = {
  originalName: string | null;
  originalResourceId: string | null;
  name: string;
  resourceId: string;
};
export type AiSearchBindingKind = Extract<
  BindingKind,
  "ai_search" | "ai_search_namespace"
>;

export function WorkerAiSearchBindingDialog({
  kind,
  workerId,
  bindingNames,
  draft,
  onChange,
  onClose,
  onBack,
}: {
  kind: AiSearchBindingKind;
  workerId: string;
  bindingNames: readonly string[];
  draft: AiSearchBindingDraft | null;
  onChange: (draft: AiSearchBindingDraft) => void;
  onClose: () => void;
  onBack: () => void;
}) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const label = kind === "ai_search" ? "AI Search" : "AI Search namespace";
  const resources = useQuery({
    queryKey: ["cloudflare-v4", kind, "binding-resources", selectedInstanceId],
    queryFn: async ({ signal }) => {
      const namespaces = await client!.aiSearch.namespaces.list(
        { account_id: selectedInstanceId!, per_page: 100 },
        { signal },
      );
      if (kind === "ai_search_namespace") {
        return {
          rows: (namespaces.result ?? []).flatMap((item) =>
            item.name ? [{ id: item.name, name: item.name }] : [],
          ),
          ambiguous: false,
        };
      }
      const pages = await Promise.all(
        (namespaces.result ?? []).flatMap((item) =>
          item.name
            ? [
                client!.aiSearch.namespaces.instances.list(
                  item.name,
                  { account_id: selectedInstanceId!, per_page: 100 },
                  { signal },
                ),
              ]
            : [],
        ),
      );
      const counts = new Map<string, number>();
      for (const page of pages)
        for (const item of page.result ?? [])
          if (item.id) counts.set(item.id, (counts.get(item.id) ?? 0) + 1);
      return {
        rows: [...counts]
          .filter(([, count]) => count === 1)
          .map(([id]) => ({ id, name: id })),
        ambiguous: [...counts.values()].some((count) => count > 1),
      };
    },
    enabled: Boolean(client && selectedInstanceId && draft),
  });
  const rows = resources.data?.rows ?? [];
  const name = draft?.name.trim() ?? "";
  const duplicate = bindingNames.some(
    (bindingName) =>
      bindingName === name && bindingName !== draft?.originalName,
  );
  const nameValid = /^[A-Za-z_$][A-Za-z0-9_$]{0,254}$/.test(name);
  const valid = Boolean(
    draft &&
    nameValid &&
    !duplicate &&
    rows.some((item) => item.id === draft.resourceId) &&
    (draft.originalName === null ||
      name !== draft.originalName ||
      draft.resourceId !== draft.originalResourceId),
  );
  const save = useMutation({
    mutationFn: async (current: AiSearchBindingDraft) => {
      if (!client || !selectedInstanceId)
        throw new Error("Dashboard session is unavailable.");
      await saveWorkerResourceBinding(
        client,
        selectedInstanceId,
        workerId,
        bindingNames,
        {
          type: kind,
          originalName: current.originalName,
          name: current.name,
          resourceId: current.resourceId,
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
      feedback.success(`${label} binding deployed.`);
    },
    onError: (error) =>
      feedback.failure(error, `Unable to deploy ${label} binding.`),
  });

  return (
    <Dialog.Root
      open={draft !== null}
      onOpenChange={(open) => {
        if (!open && !save.isPending) onClose();
      }}
    >
      <Dialog className="overflow-y-auto p-0" size="xl">
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (draft && valid) save.mutate(draft);
          }}
        >
          <WorkerBindingDialogLayout
            title={
              draft?.originalName
                ? `Edit ${label} binding`
                : `Add ${label} binding`
            }
            description={`Connect this Worker to ${label}.`}
            fields={
              <>
                <Input
                  label="Variable name"
                  description="The name used to reference this binding."
                  placeholder="MY_BINDING"
                  value={draft?.name ?? ""}
                  onChange={(event) =>
                    draft && onChange({ ...draft, name: event.target.value })
                  }
                  required
                />
                <Select
                  label={`Production ${label}`}
                  description={`The ${label} this binding is connected to.`}
                  placeholder={
                    resources.isPending
                      ? "Loading resources…"
                      : `Select ${label}`
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
                {resources.data?.ambiguous ? (
                  <p className="text-kumo-subtle">
                    Instance names repeated across namespaces are unavailable
                    for direct binding.
                  </p>
                ) : null}
                {name && !nameValid ? (
                  <p className="text-kumo-danger">
                    Use a valid JavaScript variable name (up to 255 characters).
                  </p>
                ) : null}
                {duplicate ? (
                  <p className="text-kumo-danger">
                    A binding with this name already exists.
                  </p>
                ) : null}
                {rows.length === 0 &&
                !resources.isPending &&
                !resources.error ? (
                  <p className="text-kumo-subtle">
                    No {label} resources found.
                  </p>
                ) : null}
                {resources.error ? (
                  <p className="text-kumo-danger">
                    Unable to load {label} resources.
                  </p>
                ) : null}
                {save.isError ? (
                  <p className="text-kumo-danger">{save.error.message}</p>
                ) : null}
              </>
            }
            preview={
              <CodeBlock
                className="text-kumo-default min-w-0 overflow-auto px-5 py-4 font-mono text-sm leading-5 sm:self-center"
                code={
                  kind === "ai_search_namespace"
                    ? `export default {
  async fetch(request, env) {
    const instance = env.${name || "MY_BINDING"}.get("my-instance");
    const results = await instance.search({
      query: "How does caching work?",
    });
    return Response.json(results);
  }
}`
                    : `export default {
  async fetch(request, env) {
    const results = await env.${name || "MY_BINDING"}.search({
      query: "How does caching work?",
    });
    return Response.json(results);
  }
}`
                }
                language="javascript"
              />
            }
            pending={save.isPending}
            valid={valid}
            cancelLabel={draft?.originalName ? "Cancel" : "Back"}
            submitLabel={draft?.originalName ? "Deploy" : "Add binding"}
            onCancel={draft?.originalName ? onClose : onBack}
          />
        </form>
      </Dialog>
    </Dialog.Root>
  );
}
