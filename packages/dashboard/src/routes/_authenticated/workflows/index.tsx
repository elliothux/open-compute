import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { Button } from "@cloudflare/kumo/components/button";
import { OfficialCatalog } from "../../../components/OfficialCatalog";
import { WorkflowDefinitionDialog } from "../../../components/WorkflowDefinitionDialog";
import { useAuth } from "../../../features/auth/AuthProvider";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";

export const Route = createFileRoute("/_authenticated/workflows/")({ component: WorkflowsPage });

function WorkflowsPage() {
  const { client, accountId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [createOpen, setCreateOpen] = useState(false);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const create = useMutation({
    mutationFn: (input: { name?: string; scriptName: string; className: string }) => {
      if (!input.name) throw new Error("Workflow name is required.");
      return client!.cloudflare.workflows.update(input.name, {
        account_id: accountId!,
        script_name: input.scriptName,
        class_name: input.className,
      });
    },
    onSuccess: async () => {
      setCreateOpen(false);
      setMutationError(null);
      await queryClient.invalidateQueries({ queryKey: ["cloudflare-v4", "Workflows", accountId] });
      feedback.success("Workflow created.");
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to create the Workflow.");
      feedback.failure(error, "Unable to create the Workflow.");
    },
  });
  return <OfficialCatalog
    kind="Workflows"
    description="Create definitions and manage Workflows through the official Workflows API."
    load={async (management, accountID, signal) => {
      const page = await management.cloudflare.workflows.list({ account_id: accountID }, { signal });
      return page.result.map(workflow => ({
        id: workflow.name,
        name: workflow.name,
        detail: `${workflow.script_name} / ${workflow.class_name}`,
        href: `/workflows/${encodeURIComponent(workflow.name)}`,
      }));
    }}
    remove={(management, accountID, row) => management.cloudflare.workflows.delete(row.id, { account_id: accountID })}
    primaryAction={<>
      <Button variant="primary" onClick={() => setCreateOpen(true)}>Create Workflow</Button>
      <WorkflowDefinitionDialog
        mode="create"
        open={createOpen}
        errorMessage={createOpen ? mutationError : null}
        isPending={create.isPending}
        onClose={() => { setCreateOpen(false); setMutationError(null); }}
        onSubmit={input => create.mutate(input)}
      />
    </>}
  />;
}
