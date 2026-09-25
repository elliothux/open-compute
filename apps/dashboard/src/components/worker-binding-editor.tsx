import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";
import { Select } from "@cloudflare/kumo/components/select";
import { IconEdit, IconPlus, IconTrash, IconX } from "@tabler/icons-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import type { OpenComputeJsonValue } from "@open-compute/sdk";
import { useAuth } from "../features/auth/auth-atoms";
import { useMutationFeedback } from "../features/toast/use-mutation-feedback";
import { CodeBlock } from "./code-block";
import {
  WorkerAiSearchBindingDialog,
  type AiSearchBindingDraft,
  type AiSearchBindingKind,
} from "./worker-ai-search-binding-dialog";
import { WorkerBindingGallery } from "./worker-binding-gallery";
import {
  WorkerBindingDialogLayout,
  WorkerBindingDrawerLayout,
} from "./worker-binding-layout";
import {
  WorkerDynamicWorkersBindingDrawer,
  type DynamicWorkersBindingDraft,
} from "./worker-dynamic-workers-binding-drawer";
import {
  WorkerR2BindingDrawer,
  type R2BindingDraft,
} from "./worker-r2-binding-drawer";
import {
  invalidateWorkerBindingQueries,
  resourceBindingLabels,
  saveWorkerResourceBinding,
  type BindingKind,
  type ResourceBindingChange,
} from "./worker-resource-binding-save";
import { WorkerServiceBindingDrawer } from "./worker-service-binding-drawer";
import {
  WorkerSimpleBindingDialog,
  type SimpleBindingDraft,
  type SimpleBindingKind,
} from "./worker-simple-binding-dialog";
import {
  WorkerTwoColumnBindingDialog,
  type TwoColumnBindingDraft,
  type TwoColumnBindingKind,
} from "./worker-two-column-binding-dialog";

type Binding = {
  name?: string;
  type: string;
  namespace_id?: string;
  bucket_name?: string;
  database_id?: string;
  class_name?: string;
  queue_name?: string;
  service?: string;
  entrypoint?: string | null;
  props?: { readonly [key: string]: OpenComputeJsonValue };
  index_name?: string;
  instance_name?: string;
  namespace?: string;
};
type Draft = {
  originalName: string | null;
  originalNamespaceId: string | null;
  name: string;
  namespaceId: string;
};

const deleteCopy = {
  kv_namespace: ["KV", "KV namespace", true],
  r2_bucket: ["R2", "R2 bucket", true],
  d1: ["D1", "D1 database", true],
  durable_object_namespace: [
    "Durable Object",
    "Durable Object namespace",
    true,
  ],
  queue: ["Queue", "Queue", true],
  service: ["Service", "Worker service", true],
  vectorize: ["Vectorize index", "Vectorize index", true],
  images: ["Images", "Images binding", false],
  ai: ["Workers AI", "Workers AI binding", false],
  version_metadata: ["Version metadata", "Version metadata binding", false],
  worker_loader: ["Dynamic Workers", "Dynamic Workers binding", false],
  ai_search: ["AI Search", "AI Search instance", true],
  ai_search_namespace: ["AI Search namespace", "AI Search namespace", true],
} satisfies Record<BindingKind, readonly [string, string, boolean]>;

function isBindingKind(type: string): type is BindingKind {
  return Object.hasOwn(deleteCopy, type);
}

export function WorkerBindingEditor({
  workerId,
  bindings,
  secretNames,
}: {
  workerId: string;
  bindings: readonly Binding[];
  secretNames: readonly string[];
}) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [draft, setDraft] = useState<Draft | null>(null);
  const [r2Draft, setR2Draft] = useState<R2BindingDraft | null>(null);
  const [twoColumnBinding, setTwoColumnBinding] = useState<{
    kind: TwoColumnBindingKind;
    draft: TwoColumnBindingDraft;
  } | null>(null);
  const [serviceDraft, setServiceDraft] =
    useState<TwoColumnBindingDraft | null>(null);
  const [simpleBinding, setSimpleBinding] = useState<{
    kind: SimpleBindingKind;
    draft: SimpleBindingDraft;
  } | null>(null);
  const [dynamicWorkersDraft, setDynamicWorkersDraft] =
    useState<DynamicWorkersBindingDraft | null>(null);
  const [aiSearchBinding, setAiSearchBinding] = useState<{
    kind: AiSearchBindingKind;
    draft: AiSearchBindingDraft;
  } | null>(null);
  const [galleryOpen, setGalleryOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<{
    name: string;
    type: BindingKind;
  } | null>(null);
  const names = [
    ...new Set(
      [...bindings.map((item) => item.name), ...secretNames].filter(
        (name): name is string => Boolean(name),
      ),
    ),
  ];
  const namespaces = useQuery({
    queryKey: ["cloudflare-v4", "kv", selectedInstanceId, "namespaces"],
    queryFn: async ({ signal }) => {
      const result: { id: string; title: string }[] = [];
      for await (const namespace of client!.kv.namespaces.list(
        { account_id: selectedInstanceId!, per_page: 1000, order: "title" },
        { signal },
      ))
        result.push({ id: namespace.id, title: namespace.title });
      return result;
    },
    enabled: Boolean(client && selectedInstanceId),
  });
  const bindingRows = bindings.filter(
    (item) => !["plain_text", "json", "secret_text"].includes(item.type),
  );
  const duplicate =
    draft !== null &&
    names.some(
      (name) => name === draft.name.trim() && name !== draft.originalName,
    );
  const valid = Boolean(
    draft?.name.trim() &&
    draft.namespaceId &&
    !duplicate &&
    (draft.originalName === null ||
      draft.name.trim() !== draft.originalName ||
      draft.namespaceId !== draft.originalNamespaceId) &&
    namespaces.data?.some((item) => item.id === draft.namespaceId),
  );

  const save = useMutation({
    mutationFn: async (change: ResourceBindingChange) => {
      if (!client || !selectedInstanceId)
        throw new Error("Dashboard session is unavailable.");
      await saveWorkerResourceBinding(
        client,
        selectedInstanceId,
        workerId,
        names,
        change,
      );
    },
    onSuccess: async () => {
      setDraft(null);
      setR2Draft(null);
      setServiceDraft(null);
      setDeleteTarget(null);
      await invalidateWorkerBindingQueries(
        queryClient,
        selectedInstanceId!,
        workerId,
      );
      feedback.success("Binding deployed.");
    },
    onError: (error) => feedback.failure(error, "Unable to deploy binding."),
  });

  const chooseBinding = (kind: BindingKind) => {
    setGalleryOpen(false);
    save.reset();
    if (kind === "kv_namespace") {
      setDraft({
        originalName: null,
        originalNamespaceId: null,
        name: "",
        namespaceId: "",
      });
    } else if (kind === "r2_bucket") {
      setR2Draft({
        originalName: null,
        originalBucketName: null,
        name: "",
        bucketName: "",
      });
    } else if (
      kind === "images" ||
      kind === "ai" ||
      kind === "version_metadata"
    ) {
      setSimpleBinding({
        kind,
        draft: { originalName: null, name: "" },
      });
    } else if (kind === "worker_loader") {
      setDynamicWorkersDraft({ originalName: null, name: "" });
    } else if (kind === "ai_search" || kind === "ai_search_namespace") {
      setAiSearchBinding({
        kind,
        draft: {
          originalName: null,
          originalResourceId: null,
          name: "",
          resourceId: "",
        },
      });
    } else if (kind === "service") {
      const empty = {
        originalName: null,
        originalResourceId: null,
        name: "",
        resourceId: "",
      };
      setServiceDraft({
        ...empty,
        entrypoint: "",
        originalEntrypoint: "",
        propsText: "",
      });
    } else {
      setTwoColumnBinding({
        kind,
        draft: {
          originalName: null,
          originalResourceId: null,
          name: "",
          resourceId: "",
        },
      });
    }
  };

  return (
    <>
      <div className="flex justify-end">
        <Button variant="secondary" onClick={() => setGalleryOpen(true)}>
          <IconPlus size={16} /> Add binding
        </Button>
      </div>
      <WorkerBindingGallery
        open={galleryOpen}
        onClose={() => setGalleryOpen(false)}
        onChoose={chooseBinding}
      />
      <div className="bg-kumo-base ring-kumo-line overflow-hidden rounded-lg ring">
        <div className="border-kumo-line text-kumo-subtle grid grid-cols-4 gap-2 border-b px-4 py-2 text-xs font-medium">
          <span>Type</span>
          <span>Name</span>
          <span>Value</span>
          <span>Actions</span>
        </div>
        {bindingRows.length ? (
          bindingRows.map((binding) => {
            const bindingType = isBindingKind(binding.type)
              ? binding.type
              : null;
            return (
              <div
                key={binding.name}
                className="border-kumo-line grid grid-cols-4 items-center gap-2 border-b px-4 py-2 last:border-0"
              >
                <span>
                  {bindingType
                    ? resourceBindingLabels[bindingType]
                    : binding.type}
                </span>
                <code className="truncate text-xs">{binding.name}</code>
                {binding.type === "kv_namespace" && binding.namespace_id ? (
                  <Link
                    className="text-kumo-brand truncate hover:underline"
                    to="/kv/$namespaceId"
                    params={{ namespaceId: binding.namespace_id }}
                  >
                    {namespaces.data?.find(
                      (item) => item.id === binding.namespace_id,
                    )?.title ?? binding.namespace_id}
                  </Link>
                ) : binding.type === "r2_bucket" && binding.bucket_name ? (
                  <Link
                    className="text-kumo-brand truncate hover:underline"
                    to="/r2/$bucketId"
                    params={{ bucketId: binding.bucket_name }}
                    search={{ prefix: "" }}
                  >
                    {binding.bucket_name}
                  </Link>
                ) : binding.type === "d1" && binding.database_id ? (
                  <Link
                    className="text-kumo-brand truncate hover:underline"
                    to="/d1/$databaseId"
                    params={{ databaseId: binding.database_id }}
                  >
                    {binding.database_id}
                  </Link>
                ) : binding.type === "durable_object_namespace" ? (
                  <span className="truncate">{binding.class_name ?? "—"}</span>
                ) : binding.type === "queue" && binding.queue_name ? (
                  <span className="truncate">{binding.queue_name}</span>
                ) : binding.type === "service" && binding.service ? (
                  <Link
                    className="text-kumo-brand truncate hover:underline"
                    to="/workers/$workerId"
                    params={{ workerId: binding.service }}
                  >
                    {binding.service}
                  </Link>
                ) : binding.type === "vectorize" && binding.index_name ? (
                  <Link
                    className="text-kumo-brand truncate hover:underline"
                    to="/vectorize/$indexName"
                    params={{ indexName: binding.index_name }}
                    search={{ tab: "overview" }}
                  >
                    {binding.index_name}
                  </Link>
                ) : binding.type === "images" ? (
                  <span className="text-kumo-subtle">Built-in</span>
                ) : binding.type === "ai" ? (
                  <span className="text-kumo-subtle">Built-in</span>
                ) : binding.type === "version_metadata" ? (
                  <span className="text-kumo-subtle">Built-in</span>
                ) : binding.type === "worker_loader" ? (
                  <span className="text-kumo-subtle">Built-in</span>
                ) : binding.type === "ai_search" ? (
                  <span className="truncate">{binding.instance_name}</span>
                ) : binding.type === "ai_search_namespace" ? (
                  <span className="truncate">{binding.namespace}</span>
                ) : (
                  <span>—</span>
                )}
                <span className="flex justify-end">
                  {bindingType ? (
                    <>
                      <Button
                        variant="ghost"
                        shape="square"
                        aria-label={`Edit ${binding.name}`}
                        onClick={() => {
                          save.reset();
                          if (binding.type === "kv_namespace")
                            setDraft({
                              originalName: binding.name ?? null,
                              originalNamespaceId: binding.namespace_id ?? null,
                              name: binding.name ?? "",
                              namespaceId: binding.namespace_id ?? "",
                            });
                          else if (binding.type === "r2_bucket")
                            setR2Draft({
                              originalName: binding.name ?? null,
                              originalBucketName: binding.bucket_name ?? null,
                              name: binding.name ?? "",
                              bucketName: binding.bucket_name ?? "",
                            });
                          else if (
                            binding.type === "d1" ||
                            binding.type === "durable_object_namespace" ||
                            binding.type === "queue" ||
                            binding.type === "vectorize"
                          ) {
                            const resourceId =
                              binding.type === "d1"
                                ? binding.database_id
                                : binding.type === "durable_object_namespace"
                                  ? binding.class_name
                                  : binding.type === "queue"
                                    ? binding.queue_name
                                    : binding.index_name;
                            setTwoColumnBinding({
                              kind: binding.type,
                              draft: {
                                originalName: binding.name ?? null,
                                originalResourceId: resourceId ?? null,
                                name: binding.name ?? "",
                                resourceId: resourceId ?? "",
                              },
                            });
                          } else if (binding.type === "service")
                            setServiceDraft({
                              originalName: binding.name ?? null,
                              originalResourceId: binding.service ?? null,
                              name: binding.name ?? "",
                              resourceId: binding.service ?? "",
                              entrypoint: binding.entrypoint ?? "",
                              originalEntrypoint: binding.entrypoint ?? "",
                              propsText: binding.props
                                ? JSON.stringify(binding.props, null, 2)
                                : "",
                              ...(binding.props
                                ? {
                                    originalPropsText: JSON.stringify(
                                      binding.props,
                                    ),
                                  }
                                : {}),
                            });
                          else if (
                            binding.type === "ai_search" ||
                            binding.type === "ai_search_namespace"
                          ) {
                            const resourceId =
                              binding.type === "ai_search"
                                ? binding.instance_name
                                : binding.namespace;
                            setAiSearchBinding({
                              kind: binding.type,
                              draft: {
                                originalName: binding.name ?? null,
                                originalResourceId: resourceId ?? null,
                                name: binding.name ?? "",
                                resourceId: resourceId ?? "",
                              },
                            });
                          } else if (
                            binding.type === "version_metadata" ||
                            binding.type === "ai" ||
                            binding.type === "images"
                          )
                            setSimpleBinding({
                              kind: binding.type,
                              draft: {
                                originalName: binding.name ?? null,
                                name: binding.name ?? "",
                              },
                            });
                          else if (binding.type === "worker_loader")
                            setDynamicWorkersDraft({
                              originalName: binding.name ?? null,
                              name: binding.name ?? "",
                            });
                        }}
                      >
                        <IconEdit size={16} />
                      </Button>
                      <Button
                        variant="ghost"
                        shape="square"
                        aria-label={`Delete ${binding.name}`}
                        onClick={() => {
                          save.reset();
                          if (binding.name)
                            setDeleteTarget({
                              name: binding.name,
                              type: bindingType,
                            });
                        }}
                      >
                        <IconTrash size={16} />
                      </Button>
                    </>
                  ) : null}
                </span>
              </div>
            );
          })
        ) : (
          <p className="text-kumo-subtle px-4 py-4">No connected bindings.</p>
        )}
      </div>
      <Dialog.Root
        open={draft !== null && draft.originalName === null}
        onOpenChange={(open) => {
          if (!open && !save.isPending) setDraft(null);
        }}
      >
        <Dialog className="p-0" size="xl">
          <form
            onSubmit={(event) => {
              event.preventDefault();
              if (draft && valid)
                save.mutate({ type: "kv_namespace", ...draft });
            }}
          >
            <WorkerBindingDialogLayout
              title="Add KV namespace binding"
              description="Connect this Worker to a Production KV namespace."
              fields={
                <>
                  <Input
                    label="Variable name"
                    description="The name used to reference this binding."
                    placeholder="MY_BINDING"
                    value={draft?.name ?? ""}
                    onChange={(event) =>
                      setDraft(
                        (current) =>
                          current && { ...current, name: event.target.value },
                      )
                    }
                    required
                  />
                  <div className="grid gap-2">
                    <span className="font-medium">Production</span>
                    <Select
                      aria-label="KV namespace"
                      className="w-full min-w-0 overflow-hidden"
                      value={draft?.namespaceId ?? ""}
                      placeholder={
                        namespaces.isPending
                          ? "Loading namespaces…"
                          : "Select KV namespace"
                      }
                      disabled={
                        namespaces.isPending || Boolean(namespaces.error)
                      }
                      renderValue={(value) =>
                        namespaces.data?.find((item) => item.id === value)
                          ?.title ?? value
                      }
                      onValueChange={(value) =>
                        setDraft(
                          (current) =>
                            current && { ...current, namespaceId: value ?? "" },
                        )
                      }
                    >
                      <Select.Option value="">
                        Select KV namespace
                      </Select.Option>
                      {(namespaces.data ?? []).map((item) => (
                        <Select.Option key={item.id} value={item.id}>
                          {item.title}
                        </Select.Option>
                      ))}
                    </Select>
                    <span className="text-kumo-subtle">
                      The KV namespace this binding is connected to.
                    </span>
                  </div>
                  {duplicate ? (
                    <p className="text-kumo-danger">
                      A binding with this name already exists.
                    </p>
                  ) : null}
                  {namespaces.error ? (
                    <p className="text-kumo-danger">
                      Unable to load KV namespaces.
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
                  code={`export default {\n  async fetch(request, env, ctx) {\n    await env.${draft?.name.trim() || "MY_BINDING"}.put('KEY', 'VALUE');\n  },\n}`}
                  language="javascript"
                />
              }
              pending={save.isPending}
              valid={valid}
              cancelLabel="Back"
              submitLabel="Add binding"
              onCancel={() => {
                setDraft(null);
                setGalleryOpen(true);
              }}
            />
          </form>
        </Dialog>
      </Dialog.Root>
      <WorkerBindingDrawerLayout
        open={draft !== null && draft.originalName !== null}
        pending={save.isPending}
        valid={valid}
        title="KV namespace"
        description="Bind a KV namespace to your Worker."
        onClose={() => setDraft(null)}
        onSubmit={(event) => {
          event.preventDefault();
          if (draft && valid) save.mutate({ type: "kv_namespace", ...draft });
        }}
      >
        <Input
          label="Variable name"
          description="The name used to reference this binding."
          value={draft?.name ?? ""}
          onChange={(event) =>
            setDraft(
              (current) => current && { ...current, name: event.target.value },
            )
          }
          required
        />
        <Select
          label="Production KV namespace"
          description="The KV namespace this binding is connected to."
          placeholder="Select KV namespace"
          value={draft?.namespaceId ?? ""}
          items={(namespaces.data ?? []).map((item) => ({
            label: item.title,
            value: item.id,
          }))}
          disabled={namespaces.isPending || Boolean(namespaces.error)}
          onValueChange={(value) =>
            setDraft(
              (current) => current && { ...current, namespaceId: value ?? "" },
            )
          }
        />
        {duplicate ? (
          <p className="text-kumo-danger">
            A binding with this name already exists.
          </p>
        ) : null}
        {save.isError ? (
          <p className="text-kumo-danger">{save.error.message}</p>
        ) : null}
      </WorkerBindingDrawerLayout>
      <WorkerR2BindingDrawer
        workerId={workerId}
        bindingNames={names}
        draft={r2Draft}
        onChange={setR2Draft}
        onClose={() => setR2Draft(null)}
      />
      <WorkerTwoColumnBindingDialog
        kind={twoColumnBinding?.kind ?? "d1"}
        workerId={workerId}
        bindingNames={names}
        draft={twoColumnBinding?.draft ?? null}
        onChange={(draft) =>
          setTwoColumnBinding((current) =>
            current ? { ...current, draft } : current,
          )
        }
        onClose={() => setTwoColumnBinding(null)}
        onBack={() => {
          setTwoColumnBinding(null);
          setGalleryOpen(true);
        }}
      />
      <WorkerServiceBindingDrawer
        workerId={workerId}
        bindingNames={names}
        draft={serviceDraft}
        onChange={setServiceDraft}
        onClose={() => setServiceDraft(null)}
      />
      <WorkerSimpleBindingDialog
        kind={simpleBinding?.kind ?? "ai"}
        workerId={workerId}
        bindingNames={names}
        draft={simpleBinding?.draft ?? null}
        onChange={(draft) =>
          setSimpleBinding((current) =>
            current ? { ...current, draft } : current,
          )
        }
        onClose={() => setSimpleBinding(null)}
        onBack={() => {
          setSimpleBinding(null);
          setGalleryOpen(true);
        }}
      />
      <WorkerDynamicWorkersBindingDrawer
        workerId={workerId}
        bindingNames={names}
        draft={dynamicWorkersDraft}
        onChange={setDynamicWorkersDraft}
        onClose={() => setDynamicWorkersDraft(null)}
      />
      <WorkerAiSearchBindingDialog
        kind={aiSearchBinding?.kind ?? "ai_search"}
        workerId={workerId}
        bindingNames={names}
        draft={aiSearchBinding?.draft ?? null}
        onChange={(draft) =>
          setAiSearchBinding((current) =>
            current ? { ...current, draft } : current,
          )
        }
        onClose={() => setAiSearchBinding(null)}
        onBack={() => {
          setAiSearchBinding(null);
          setGalleryOpen(true);
        }}
      />
      <Dialog.Root
        open={deleteTarget !== null}
        role="alertdialog"
        onOpenChange={(open) => {
          if (!open && !save.isPending) setDeleteTarget(null);
        }}
      >
        <Dialog className="p-0" size="xl">
          <div className="flex items-center justify-between px-8 py-4">
            <Dialog.Title className="text-xl font-medium">
              Delete {deleteTarget ? deleteCopy[deleteTarget.type][0] : ""}{" "}
              binding?
            </Dialog.Title>
            <Button
              variant="ghost"
              shape="square"
              aria-label="Close"
              disabled={save.isPending}
              onClick={() => setDeleteTarget(null)}
            >
              <IconX size={16} />
            </Button>
          </div>
          <div className="min-h-32 px-8 pt-2">
            <Dialog.Description>
              This Worker will no longer be able to access resources or services
              connected to <strong>{deleteTarget?.name}</strong>. The{" "}
              {deleteTarget ? deleteCopy[deleteTarget.type][1] : ""}{" "}
              {deleteTarget && deleteCopy[deleteTarget.type][2]
                ? "and its data will remain."
                : "will no longer be available to this Worker."}
            </Dialog.Description>
            {save.isError ? (
              <p className="text-kumo-danger mt-3">{save.error.message}</p>
            ) : null}
          </div>
          <div className="bg-kumo-recessed flex justify-end gap-2 px-8 py-4">
            <Button
              variant="ghost"
              disabled={save.isPending}
              onClick={() => setDeleteTarget(null)}
            >
              Cancel
            </Button>
            <Button
              variant="destructive"
              disabled={save.isPending}
              onClick={() => {
                if (deleteTarget)
                  save.mutate({
                    type: deleteTarget.type,
                    originalName: deleteTarget.name,
                    remove: true,
                  });
              }}
            >
              {save.isPending ? "Deploying…" : "Delete and deploy"}
            </Button>
          </div>
        </Dialog>
      </Dialog.Root>
    </>
  );
}
