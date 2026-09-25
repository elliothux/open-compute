import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { IconArrowLeft } from "@tabler/icons-react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { CloudflareProductIcon } from "../../../components/cloudflare-product-icons";
import { CreateStepper } from "../../../components/create-stepper";
import { useAuth } from "../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";

export const Route = createFileRoute("/_authenticated/workflows/new")({
  component: CreateWorkflowPage,
});

function CreateWorkflowPage() {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [step, setStep] = useState<"start" | "setup">("start");
  const [name, setName] = useState("");
  const [scriptName, setScriptName] = useState("");
  const [className, setClassName] = useState("");
  const [successRetention, setSuccessRetention] = useState("");
  const [errorRetention, setErrorRetention] = useState("");
  const retentionValid = [successRetention, errorRetention].every(
    (value) =>
      value === "" || (Number.isInteger(Number(value)) && Number(value) > 0),
  );
  const valid = Boolean(
    name.trim() && scriptName.trim() && className.trim() && retentionValid,
  );
  const create = useMutation({
    mutationFn: () => {
      if (!client || !selectedInstanceId || !valid) {
        throw new Error("Complete the required workflow fields.");
      }
      return client.workflows.update(name.trim(), {
        account_id: selectedInstanceId,
        script_name: scriptName.trim(),
        class_name: className.trim(),
        ...(successRetention || errorRetention
          ? {
              default_retention: {
                ...(successRetention
                  ? { success_retention: Number(successRetention) }
                  : {}),
                ...(errorRetention
                  ? { error_retention: Number(errorRetention) }
                  : {}),
              },
            }
          : {}),
      });
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: ["cloudflare-v4", "workflows", selectedInstanceId],
      });
      feedback.success("Workflow created.");
      await navigate({
        to: "/workflows/$workflowId",
        params: { workflowId: name.trim() },
      });
    },
    onError: (error) =>
      feedback.failure(error, "Unable to create the workflow."),
  });

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (valid && !create.isPending) create.mutate();
  }

  return (
    <CreateStepper
      title="Create workflow"
      steps={["Choose a method", "Set up workflow"]}
      current={step === "start" ? 0 : 1}
      onClose={() => void navigate({ to: "/workflows" })}
    >
      {step === "start" ? (
        <section className="mx-auto w-full max-w-xl p-7">
          <h1 className="text-lg leading-5 font-medium">
            Choose how to get started
          </h1>
          <p className="text-kumo-subtle mt-1.5">
            Create a Workflow definition from a Worker script and exported class
            already available on this installation.
          </p>
          <button
            type="button"
            className="bg-kumo-base ring-kumo-line hover:bg-kumo-tint mt-6 flex min-h-16 w-full items-center gap-3 rounded-lg px-3 text-left ring focus-visible:outline-2 focus-visible:outline-offset-2"
            onClick={() => setStep("setup")}
          >
            <span className="bg-kumo-info-tint text-kumo-brand flex size-9 shrink-0 items-center justify-center rounded-lg">
              <CloudflareProductIcon product="Workflows" size={20} />
            </span>
            <span className="grid gap-0.5">
              <span className="font-medium">Use an existing Worker class</span>
              <span className="text-kumo-subtle">
                Register a Workflow without Git connection or automatic
                deployment.
              </span>
            </span>
          </button>
        </section>
      ) : (
        <form onSubmit={submit} className="mx-auto w-full max-w-xl">
          <section className="p-7">
            <h1 className="text-lg leading-5 font-medium">
              Set up your workflow
            </h1>
            <p className="text-kumo-subtle mt-1.5">
              Register an existing Worker class as a Workflow definition. This
              does not deploy the Worker.
            </p>
            <div className="mt-6 grid gap-5">
              <Input
                label="Workflow name"
                value={name}
                onChange={(event) => setName(event.target.value)}
                autoComplete="off"
              />
              <Input
                label="Worker script"
                value={scriptName}
                onChange={(event) => setScriptName(event.target.value)}
                autoComplete="off"
              />
              <Input
                label="Exported class"
                value={className}
                onChange={(event) => setClassName(event.target.value)}
                autoComplete="off"
              />
              <div className="grid gap-5 sm:grid-cols-2">
                <Input
                  label="Success retention (ms)"
                  type="number"
                  min={1}
                  value={successRetention}
                  onChange={(event) => setSuccessRetention(event.target.value)}
                />
                <Input
                  label="Error retention (ms)"
                  type="number"
                  min={1}
                  value={errorRetention}
                  onChange={(event) => setErrorRetention(event.target.value)}
                />
              </div>
            </div>
            {create.error ? (
              <p className="text-kumo-danger mt-4" role="alert">
                {create.error instanceof Error
                  ? create.error.message
                  : "Unable to create the workflow."}
              </p>
            ) : null}
          </section>
          <div className="border-kumo-line bg-kumo-canvas sticky bottom-0 flex items-center justify-between gap-2 border-t py-2">
            <Button
              type="button"
              variant="secondary"
              icon={<IconArrowLeft size={16} />}
              disabled={create.isPending}
              onClick={() => setStep("start")}
            >
              Back
            </Button>
            <Button
              type="submit"
              variant="primary"
              disabled={
                !valid || !client || !selectedInstanceId || create.isPending
              }
            >
              {create.isPending ? "Creating…" : "Create workflow"}
            </Button>
          </div>
        </form>
      )}
    </CreateStepper>
  );
}
