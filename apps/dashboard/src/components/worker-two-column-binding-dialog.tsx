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

export type TwoColumnBindingDraft = {
  originalName: string | null;
  originalResourceId: string | null;
  name: string;
  resourceId: string;
  entrypoint?: string;
  originalEntrypoint?: string;
  propsText?: string;
  originalPropsText?: string;
};
export type TwoColumnBindingKind = Extract<
  BindingKind,
  "d1" | "durable_object_namespace" | "queue" | "vectorize"
>;

type BindingResource = { id: string; name: string; namespaceId?: string };

export function WorkerTwoColumnBindingDialog({
  kind,
  workerId,
  bindingNames,
  draft,
  onChange,
  onClose,
  onBack,
}: {
  kind: TwoColumnBindingKind;
  workerId: string;
  bindingNames: readonly string[];
  draft: TwoColumnBindingDraft | null;
  onChange: (draft: TwoColumnBindingDraft) => void;
  onClose: () => void;
  onBack: () => void;
}) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const resources = useQuery({
    queryKey: ["cloudflare-v4", kind, "binding-resources", selectedInstanceId],
    queryFn: async ({ signal }): Promise<BindingResource[]> => {
      if (kind === "d1") {
        const response = await client!.d1.database.list(
          { account_id: selectedInstanceId! },
          { signal },
        );
        return (response.result ?? []).flatMap((item) =>
          item.uuid && item.name ? [{ id: item.uuid, name: item.name }] : [],
        );
      }
      if (kind === "durable_object_namespace") {
        const rows: { id: string; name: string; namespaceId: string }[] = [];
        let cursor: string | undefined;
        do {
          const response = await client!.openCompute.durableObjects.list(
            selectedInstanceId!,
            { signal, query: { per_page: 100, ...(cursor ? { cursor } : {}) } },
          );
          rows.push(
            ...response.items.flatMap((item) =>
              item.script_name === workerId && item.state === "ready"
                ? [
                    {
                      id: item.class_name,
                      name: item.class_name,
                      namespaceId: item.id,
                    },
                  ]
                : [],
            ),
          );
          cursor = response.next_cursor;
        } while (cursor);
        return rows;
      }
      if (kind === "vectorize") {
        const response = await client!.vectorize.indexes.list(
          { account_id: selectedInstanceId! },
          { signal },
        );
        return (response.result ?? []).flatMap((item) =>
          item.name ? [{ id: item.name, name: item.name }] : [],
        );
      }
      const response = await client!.queues.list(
        { account_id: selectedInstanceId! },
        { signal },
      );
      return (response.result ?? []).flatMap((item) =>
        item.queue_name ? [{ id: item.queue_name, name: item.queue_name }] : [],
      );
    },
    enabled: Boolean(client && selectedInstanceId && draft),
  });
  const rows = resources.data ?? [];
  const label =
    kind === "d1"
      ? "D1 database"
      : kind === "durable_object_namespace"
        ? "Durable Object"
        : kind === "queue"
          ? "Queue"
          : "Vectorize index";
  const duplicate =
    draft !== null &&
    bindingNames.some(
      (name) => name === draft.name.trim() && name !== draft.originalName,
    );
  const valid = Boolean(
    draft?.name.trim() &&
    draft.resourceId &&
    !duplicate &&
    rows.some((item) => item.id === draft.resourceId) &&
    (draft.originalName === null ||
      draft.name.trim() !== draft.originalName ||
      draft.resourceId !== draft.originalResourceId),
  );
  const save = useMutation({
    mutationFn: async (current: TwoColumnBindingDraft) => {
      if (!client || !selectedInstanceId)
        throw new Error("Dashboard session is unavailable.");
      const selectedNamespaceId =
        rows.find((item) => item.id === current.resourceId)?.namespaceId ?? "";
      if (kind === "durable_object_namespace" && !selectedNamespaceId)
        throw new Error("Selected Durable Object is unavailable.");
      await saveWorkerResourceBinding(
        client,
        selectedInstanceId,
        workerId,
        bindingNames,
        kind === "d1"
          ? {
              type: "d1",
              originalName: current.originalName,
              name: current.name,
              databaseId: current.resourceId,
            }
          : kind === "durable_object_namespace"
            ? {
                type: "durable_object_namespace",
                originalName: current.originalName,
                name: current.name,
                className: current.resourceId,
                namespaceId: selectedNamespaceId,
              }
            : kind === "queue"
              ? {
                  type: "queue",
                  originalName: current.originalName,
                  name: current.name,
                  queueName: current.resourceId,
                }
              : {
                  type: "vectorize",
                  originalName: current.originalName,
                  name: current.name,
                  indexName: current.resourceId,
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
            description={`Connect this Worker to a ${label}.`}
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
                {rows.length === 0 && !resources.isPending ? (
                  <p className="text-kumo-subtle">
                    {kind === "durable_object_namespace"
                      ? "No ready Durable Object classes exported by this Worker."
                      : `No ${label} resources found.`}
                  </p>
                ) : null}
                {duplicate ? (
                  <p className="text-kumo-danger">
                    A binding with this name already exists.
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
                className="text-kumo-subtle min-w-0 overflow-auto px-5 py-4 text-xs sm:self-center"
                code={
                  kind === "d1"
                    ? `export default {\n  async fetch(request, env) {\n    const result = await env.${draft?.name.trim() || "MY_BINDING"}.prepare(\n      "SELECT * FROM [order] LIMIT 100",\n    ).run();\n    return new Response(JSON.stringify(result));\n  },\n}`
                    : kind === "durable_object_namespace"
                      ? `export default {\n  async fetch(request, env, ctx) {\n    const id = env.${draft?.name.trim() || "MY_BINDING"}.idFromName(\n      new URL(request.url).pathname\n    );\n  },\n};`
                      : kind === "queue"
                        ? `export default {\n  async fetch(req, env) {\n    await env.${draft?.name.trim() || "MY_BINDING"}.send({\n      url: req.url,\n      method: req.method,\n      headers: Object.fromEntries(req.headers),\n    });\n    return new Response('Sent!');\n  },\n}`
                        : `export default {\n  async fetch(request, env) {\n    const queryVector = [32.4, 6.55, 11.2, 10.3, 87.9];\n    const matches = await env.${draft?.name.trim() || "MY_BINDING"}.query(queryVector);\n    return Response.json(matches);\n  },\n}`
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
