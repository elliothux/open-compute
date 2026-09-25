import { Input } from "@cloudflare/kumo/components/input";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useAuth } from "../features/auth/auth-atoms";
import { useMutationFeedback } from "../features/toast/use-mutation-feedback";
import { WorkerBindingDrawerLayout } from "./worker-binding-layout";
import {
  invalidateWorkerBindingQueries,
  saveWorkerResourceBinding,
} from "./worker-resource-binding-save";

export type DynamicWorkersBindingDraft = {
  originalName: string | null;
  name: string;
};

export function WorkerDynamicWorkersBindingDrawer({
  workerId,
  bindingNames,
  draft,
  onChange,
  onClose,
}: {
  workerId: string;
  bindingNames: readonly string[];
  draft: DynamicWorkersBindingDraft | null;
  onChange: (draft: DynamicWorkersBindingDraft) => void;
  onClose: () => void;
}) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const name = draft?.name.trim() ?? "";
  const duplicate = bindingNames.some(
    (bindingName) =>
      bindingName === name && bindingName !== draft?.originalName,
  );
  const nameValid =
    /^[A-Za-z_$][A-Za-z0-9_$]{0,63}$/.test(name) &&
    !name.startsWith("OPEN_COMPUTE_") &&
    !name.startsWith("__");
  const valid = Boolean(
    draft &&
    nameValid &&
    !duplicate &&
    (draft.originalName === null || name !== draft.originalName),
  );
  const save = useMutation({
    mutationFn: async (current: DynamicWorkersBindingDraft) => {
      if (!client || !selectedInstanceId)
        throw new Error("Dashboard session is unavailable.");
      await saveWorkerResourceBinding(
        client,
        selectedInstanceId,
        workerId,
        bindingNames,
        { type: "worker_loader", ...current },
      );
    },
    onSuccess: async () => {
      onClose();
      await invalidateWorkerBindingQueries(
        queryClient,
        selectedInstanceId!,
        workerId,
      );
      feedback.success("Dynamic Workers binding deployed.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to deploy Dynamic Workers binding."),
  });

  return (
    <WorkerBindingDrawerLayout
      open={draft !== null}
      pending={save.isPending}
      valid={valid}
      title="Dynamic Workers"
      description="Add a Dynamic Workers binding."
      docsHref="https://developers.cloudflare.com/dynamic-workers/api-reference/"
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
      {name && !nameValid ? (
        <p className="text-kumo-danger">
          Use a valid JavaScript variable name (up to 64 characters).
        </p>
      ) : null}
      {duplicate ? (
        <p className="text-kumo-danger">
          A binding with this name already exists.
        </p>
      ) : null}
      {save.isError ? (
        <p className="text-kumo-danger">{save.error.message}</p>
      ) : null}
    </WorkerBindingDrawerLayout>
  );
}
