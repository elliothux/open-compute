import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useAuth } from "../features/auth/auth-atoms";
import { useMutationFeedback } from "../features/toast/use-mutation-feedback";
import { CodeBlock } from "./code-block";
import { WorkerBindingDialogLayout } from "./worker-binding-layout";
import {
  invalidateWorkerBindingQueries,
  saveWorkerResourceBinding,
  type BindingKind,
} from "./worker-resource-binding-save";

export type SimpleBindingKind = Extract<
  BindingKind,
  "ai" | "images" | "version_metadata"
>;
export type SimpleBindingDraft = {
  originalName: string | null;
  name: string;
};

const config = {
  ai: {
    label: "Workers AI",
    description:
      "Connect this Worker to Markdown Conversion through Workers AI.",
    success: "Workers AI binding deployed.",
    failure: "Unable to deploy Workers AI binding.",
    maxLength: 64,
    code: (name: string) => `export default {
  async fetch(request, env) {
    const formats = await env.${name}.toMarkdown().supported();
    return Response.json(formats);
  }
}`,
  },
  images: {
    label: "Images",
    description: "Connect this Worker to the Images transformation binding.",
    success: "Images binding deployed.",
    failure: "Unable to deploy Images binding.",
    maxLength: 255,
    code: (name: string) => `export default {
  async fetch(request, env, ctx) {
    // Fetch the main image
    const image: ReadableStream = ...

    const response = (
      await env.${name}.input(image)
        .draw(...)
        .output({ format: "image/avif" })
    ).response()

    return response;
  }
}`,
  },
  version_metadata: {
    label: "version metadata",
    description: "Make this Worker's version metadata available as a binding.",
    success: "Version metadata binding deployed.",
    failure: "Unable to deploy version metadata binding.",
    maxLength: 64,
    code: (name: string) => `export default {
  async fetch(request, env, ctx) {
    const {
      id: versionId,
      tag: versionTag,
      timestamp: versionTimestamp
    } = env.${name};
  },
}`,
  },
} satisfies Record<
  SimpleBindingKind,
  {
    label: string;
    description: string;
    success: string;
    failure: string;
    maxLength: number;
    code: (name: string) => string;
  }
>;

export function WorkerSimpleBindingDialog({
  kind,
  workerId,
  bindingNames,
  draft,
  onChange,
  onClose,
  onBack,
}: {
  kind: SimpleBindingKind;
  workerId: string;
  bindingNames: readonly string[];
  draft: SimpleBindingDraft | null;
  onChange: (draft: SimpleBindingDraft) => void;
  onClose: () => void;
  onBack: () => void;
}) {
  const { client, instanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const current = config[kind];
  const name = draft?.name.trim() ?? "";
  const duplicate = bindingNames.some(
    (bindingName) =>
      bindingName === name && bindingName !== draft?.originalName,
  );
  const nameValid =
    new RegExp(`^[A-Za-z_$][A-Za-z0-9_$]{0,${current.maxLength - 1}}$`).test(
      name,
    ) &&
    (kind !== "ai" ||
      (!name.startsWith("OPEN_COMPUTE_") && !name.startsWith("__")));
  const valid = Boolean(
    draft &&
    nameValid &&
    !duplicate &&
    (draft.originalName === null || name !== draft.originalName),
  );
  const save = useMutation({
    mutationFn: async (value: SimpleBindingDraft) => {
      if (!client || !instanceId)
        throw new Error("Dashboard session is unavailable.");
      await saveWorkerResourceBinding(
        client,
        instanceId,
        workerId,
        bindingNames,
        {
          type: kind,
          ...value,
        },
      );
    },
    onSuccess: async () => {
      onClose();
      await invalidateWorkerBindingQueries(queryClient, instanceId!, workerId);
      feedback.success(current.success);
    },
    onError: (error) => feedback.failure(error, current.failure),
  });

  return (
    <Dialog.Root
      open={draft !== null}
      onOpenChange={(open) => {
        if (!open && !save.isPending) onClose();
      }}
    >
      <Dialog className="p-0" size="xl">
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (draft && valid) save.mutate(draft);
          }}
        >
          <WorkerBindingDialogLayout
            title={`${draft?.originalName ? "Edit" : "Add"} ${current.label} binding`}
            description={current.description}
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
                {name && !nameValid ? (
                  <p className="text-kumo-danger">
                    Use a valid JavaScript variable name (up to{" "}
                    {current.maxLength} characters).
                  </p>
                ) : null}
                {duplicate ? (
                  <p className="text-kumo-danger">
                    A binding with this name already exists.
                  </p>
                ) : null}
                {save.error ? (
                  <p className="text-kumo-danger">{save.error.message}</p>
                ) : null}
              </>
            }
            preview={
              <div className="grid min-w-0 content-center gap-3 px-5 py-4">
                <CodeBlock
                  className="text-kumo-default min-w-0 overflow-auto font-mono text-sm"
                  code={current.code(name || "MY_BINDING")}
                  language="javascript"
                />
                {kind === "ai" ? (
                  <p className="text-kumo-subtle text-sm">
                    This installation supports Markdown Conversion only; general
                    model inference is unavailable.
                  </p>
                ) : null}
              </div>
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
