import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import {
  IconArrowLeft,
  IconCircleCheck,
  IconCode,
  IconUpload,
} from "@tabler/icons-react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { CloudflareProductIcon } from "../../../components/cloudflare-product-icons";
import { CodeBlock } from "../../../components/code-block";
import { CreateStepper } from "../../../components/create-stepper";
import { useAuth } from "../../../features/auth/auth-atoms";

export const Route = createFileRoute("/_authenticated/workers/new")({
  component: CreateWorkerPage,
});

const helloWorld = `/**
 * Welcome to Workers! This is your first Worker.
 */
export default {
  async fetch(request, env, ctx) {
    console.info({ message: 'Hello World Worker received a request!' });
    return new Response('Hello World!');
  }
};`;

type Method = "hello" | "upload";

function CreateWorkerPage() {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [method, setMethod] = useState<Method | null>(null);
  const [name, setName] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const trimmedName = name.trim();
  const validName = /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(trimmedName);
  const deploy = useMutation({
    mutationFn: async () => {
      if (!client || !selectedInstanceId || !method || !validName) {
        throw new Error("Enter a valid Worker name.");
      }
      const moduleFile =
        method === "hello"
          ? new File([helloWorld], "index.js", {
              type: "application/javascript+module",
            })
          : file &&
            new File([file], file.name, {
              type: "application/javascript+module",
            });
      if (!moduleFile) throw new Error("Choose a JavaScript module.");
      const capabilities = await client.openCompute.capabilities.get();
      await client.workers.scripts.update(trimmedName, {
        account_id: selectedInstanceId,
        metadata: {
          main_module: moduleFile.name,
          compatibility_date: capabilities.compatibility_date.maximum,
        },
        files: [moduleFile],
      });
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: ["cloudflare-v4", "workers", selectedInstanceId],
      });
      await navigate({
        to: "/workers/$workerId",
        params: { workerId: trimmedName },
      });
    },
  });

  return (
    <CreateStepper
      title="Create an app"
      steps={
        method === null
          ? ["Select a method"]
          : ["Select a method", "Deploy Worker"]
      }
      current={method === null ? 0 : 1}
      onClose={() => void navigate({ to: "/workers" })}
    >
      {method === null ? (
        <section className="p-2">
          <h2 className="text-base font-semibold">Make something new</h2>
          <p className="text-kumo-subtle mt-1">
            Start with a simple Worker or upload your code.
          </p>
          <div className="mt-5 grid gap-2">
            <button
              type="button"
              className="border-kumo-line hover:bg-kumo-tint flex min-h-14 items-center gap-3 rounded-lg border px-3 text-left"
              onClick={() => setMethod("hello")}
            >
              <span className="bg-kumo-info-tint text-kumo-brand flex size-8 items-center justify-center rounded-lg">
                <CloudflareProductIcon product="Workers" size={17} />
              </span>
              <span>Start with Hello World!</span>
            </button>
            <button
              type="button"
              className="border-kumo-line hover:bg-kumo-tint flex min-h-14 items-center gap-3 rounded-lg border px-3 text-left"
              onClick={() => setMethod("upload")}
            >
              <span className="bg-kumo-info-tint text-kumo-brand flex size-8 items-center justify-center rounded-lg">
                <IconUpload size={17} />
              </span>
              <span>Upload a Worker module</span>
            </button>
          </div>
        </section>
      ) : (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (!deploy.isPending) deploy.mutate();
          }}
          className="bg-kumo-base ring-kumo-line overflow-hidden rounded-xl ring"
        >
          <div className="px-6 py-5">
            <h2 className="text-base font-semibold">
              {method === "hello"
                ? "Deploy Hello World"
                : "Deploy Worker module"}
            </h2>
            <p className="text-kumo-subtle mt-1">
              {method === "hello"
                ? "A simple Worker that returns 'Hello World!'. Perfect for getting started."
                : "Upload a JavaScript module to deploy directly."}
            </p>
            <div className="mt-5">
              <Input
                label="Worker name"
                placeholder="my-worker"
                value={name}
                onChange={(event) => setName(event.target.value)}
                autoFocus
              />
              {trimmedName && validName ? (
                <p className="text-kumo-success mt-1 flex items-center gap-1 text-xs">
                  <IconCircleCheck size={14} /> Valid Worker name
                </p>
              ) : null}
              {trimmedName && !validName ? (
                <p className="text-kumo-danger mt-1 text-xs">
                  Use lowercase letters, numbers and hyphens; start and end with
                  a letter or number.
                </p>
              ) : null}
            </div>
            {method === "hello" ? (
              <div className="mt-5">
                <p className="mb-2 font-medium">Worker preview</p>
                <CodeBlock
                  className="border-kumo-line bg-kumo-tint max-h-96 overflow-auto rounded-lg border p-4 text-xs leading-5"
                  code={helloWorld}
                  language="javascript"
                />
              </div>
            ) : (
              <div className="mt-5">
                <label
                  className="mb-2 block font-medium"
                  htmlFor="worker-module"
                >
                  Worker module
                </label>
                <input
                  id="worker-module"
                  type="file"
                  accept=".js,.mjs,text/javascript,application/javascript"
                  className="border-kumo-line file:bg-kumo-tint block w-full rounded-lg border p-2 file:mr-3 file:rounded file:border-0 file:px-3 file:py-1"
                  onChange={(event) => setFile(event.target.files?.[0] ?? null)}
                />
                {file ? (
                  <p className="text-kumo-subtle mt-2 flex items-center gap-1">
                    <IconCode size={15} /> {file.name} ·{" "}
                    {file.size.toLocaleString()} bytes
                  </p>
                ) : null}
              </div>
            )}
            {deploy.error ? (
              <p role="alert" className="text-kumo-danger mt-4">
                {deploy.error instanceof Error
                  ? deploy.error.message
                  : "Unable to deploy the Worker."}
              </p>
            ) : null}
          </div>
          <div className="border-kumo-line bg-kumo-tint flex items-center justify-between border-t px-5 py-3">
            <Button
              type="button"
              variant="ghost"
              onClick={() => {
                deploy.reset();
                setMethod(null);
              }}
              disabled={deploy.isPending}
            >
              <IconArrowLeft size={15} /> Back
            </Button>
            <Button
              type="submit"
              variant="primary"
              disabled={
                !validName || (method === "upload" && !file) || deploy.isPending
              }
            >
              {deploy.isPending ? "Deploying…" : "Deploy"}
            </Button>
          </div>
        </form>
      )}
    </CreateStepper>
  );
}
