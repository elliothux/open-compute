import { Input } from "@cloudflare/kumo/components/input";
import { Select } from "@cloudflare/kumo/components/select";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAuth } from "../features/auth/auth-atoms";
import { useMutationFeedback } from "../features/toast/use-mutation-feedback";
import { WorkerBindingDrawerLayout } from "./worker-binding-layout";
import {
  invalidateWorkerBindingQueries,
  saveWorkerResourceBinding,
} from "./worker-resource-binding-save";

export type R2BindingDraft = {
  originalName: string | null;
  originalBucketName: string | null;
  name: string;
  bucketName: string;
};

export function WorkerR2BindingDrawer({
  workerId,
  bindingNames,
  draft,
  onChange,
  onClose,
}: {
  workerId: string;
  bindingNames: readonly string[];
  draft: R2BindingDraft | null;
  onChange: (draft: R2BindingDraft) => void;
  onClose: () => void;
}) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const buckets = useQuery({
    queryKey: ["cloudflare-v4", "r2", selectedInstanceId, "binding-buckets"],
    queryFn: async ({ signal }) => {
      const names: string[] = [];
      let startAfter: string | undefined;
      while (true) {
        const page = await client!.r2.buckets.list(
          {
            account_id: selectedInstanceId!,
            per_page: 1000,
            ...(startAfter ? { start_after: startAfter } : {}),
          },
          { signal },
        );
        const rows = page.buckets ?? [];
        names.push(
          ...rows
            .map((item) => item.name)
            .filter((name): name is string => Boolean(name)),
        );
        if (rows.length < 1000) break;
        const last = rows.at(-1)?.name;
        if (!last || last === startAfter)
          throw new Error("R2 bucket pagination did not advance.");
        startAfter = last;
      }
      return names;
    },
    enabled: Boolean(client && selectedInstanceId && draft),
  });
  const duplicate =
    draft !== null &&
    bindingNames.some(
      (name) => name === draft.name.trim() && name !== draft.originalName,
    );
  const valid = Boolean(
    draft?.name.trim() &&
    draft.bucketName &&
    !duplicate &&
    buckets.data?.includes(draft.bucketName) &&
    (draft.originalName === null ||
      draft.name.trim() !== draft.originalName ||
      draft.bucketName !== draft.originalBucketName),
  );
  const save = useMutation({
    mutationFn: async (current: R2BindingDraft) => {
      if (!client || !selectedInstanceId)
        throw new Error("Dashboard session is unavailable.");
      await saveWorkerResourceBinding(
        client,
        selectedInstanceId,
        workerId,
        bindingNames,
        {
          type: "r2_bucket",
          originalName: current.originalName,
          name: current.name,
          bucketName: current.bucketName,
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
      feedback.success("R2 bucket binding deployed.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to deploy R2 bucket binding."),
  });

  return (
    <WorkerBindingDrawerLayout
      open={draft !== null}
      pending={save.isPending}
      valid={valid}
      title="R2 bucket"
      description="Bind an R2 bucket to interact with its data from this Worker."
      docsHref="https://developers.cloudflare.com/r2/api/workers/workers-api-reference"
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
        label="R2 bucket"
        placeholder={
          buckets.isPending ? "Loading buckets…" : "Select R2 bucket"
        }
        value={draft?.bucketName ?? ""}
        items={(buckets.data ?? []).map((name) => ({
          label: name,
          value: name,
        }))}
        disabled={buckets.isPending || Boolean(buckets.error)}
        onValueChange={(value) =>
          draft && onChange({ ...draft, bucketName: value ?? "" })
        }
      />
      {buckets.data?.length === 0 ? (
        <p className="text-kumo-subtle">No R2 buckets found.</p>
      ) : null}
      {duplicate ? (
        <p className="text-kumo-danger">
          A binding with this name already exists.
        </p>
      ) : null}
      {buckets.error ? (
        <p className="text-kumo-danger">Unable to load R2 buckets.</p>
      ) : null}
      {save.isError ? (
        <p className="text-kumo-danger">{save.error.message}</p>
      ) : null}
    </WorkerBindingDrawerLayout>
  );
}
