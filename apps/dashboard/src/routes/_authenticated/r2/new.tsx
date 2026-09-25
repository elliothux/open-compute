import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { PageHeader } from "../../../components/dashboard-page";
import { useAuth } from "../../../features/auth/auth-atoms";

export const Route = createFileRoute("/_authenticated/r2/new")({
  component: CreateR2Page,
});

const bucketNamePattern = /^[a-z0-9](?:[a-z0-9-]{1,61}[a-z0-9])$/;

function CreateR2Page() {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const validName = bucketNamePattern.test(name);
  const create = useMutation({
    mutationFn: async () => {
      if (!client || !selectedInstanceId || !validName) {
        throw new Error("Enter a valid bucket name.");
      }
      return client.r2.buckets.create({ account_id: selectedInstanceId, name });
    },
    onSuccess: async (bucket) => {
      await queryClient.invalidateQueries({
        queryKey: ["cloudflare-v4", "r2", selectedInstanceId, "buckets"],
      });
      await navigate({
        to: "/r2/$bucketId",
        params: { bucketId: bucket.name || name },
        search: { prefix: "" },
      });
    },
  });

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (validName && !create.isPending) create.mutate();
  }

  return (
    <form onSubmit={submit} className="mx-auto max-w-2xl">
      <PageHeader
        title="Create a bucket"
        description="Create an empty bucket, then upload objects or bind it to a Worker."
      />
      <div className="grid gap-6">
        <Input
          label="Bucket name"
          value={name}
          onChange={(event) => {
            setName(event.target.value);
            create.reset();
          }}
          autoComplete="off"
          aria-invalid={name.length > 0 && !validName}
          description="Bucket names are permanent."
          {...(name && !validName
            ? {
                error:
                  "Use 3–63 lowercase letters, numbers, or hyphens; start and end with a letter or number.",
              }
            : {})}
        />
        {create.error ? (
          <p className="text-kumo-danger" role="alert">
            {create.error instanceof Error
              ? create.error.message
              : "Unable to create the bucket."}
          </p>
        ) : null}
        <p className="text-kumo-subtle">
          Buckets are private by default. Bind this bucket to a Worker or use
          the S3-compatible API to access its objects.
        </p>
        <div className="border-kumo-line flex justify-end gap-2 border-t pt-6">
          <Button
            type="button"
            variant="secondary"
            onClick={() => void navigate({ to: "/r2" })}
          >
            Cancel
          </Button>
          <Button
            type="submit"
            variant="primary"
            disabled={
              !validName || create.isPending || !client || !selectedInstanceId
            }
          >
            {create.isPending ? "Creating…" : "Create bucket"}
          </Button>
        </div>
      </div>
    </form>
  );
}
