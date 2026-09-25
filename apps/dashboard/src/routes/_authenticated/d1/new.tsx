import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { PageHeader } from "../../../components/dashboard-page";
import { useAuth } from "../../../features/auth/auth-atoms";

export const Route = createFileRoute("/_authenticated/d1/new")({
  component: CreateD1Page,
});

function CreateD1Page() {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const validName = /^[A-Za-z0-9][A-Za-z0-9_-]*$/.test(name);
  const create = useMutation({
    mutationFn: async () => {
      if (!client || !selectedInstanceId || !validName) {
        throw new Error("Enter a valid database name.");
      }
      return client.d1.database.create({
        account_id: selectedInstanceId,
        name,
      });
    },
    onSuccess: async (database) => {
      await queryClient.invalidateQueries({
        queryKey: ["cloudflare-v4", "d1", selectedInstanceId, "databases"],
      });
      if (database.uuid) {
        await navigate({
          to: "/d1/$databaseId",
          params: { databaseId: database.uuid },
        });
      } else {
        await navigate({ to: "/d1" });
      }
    },
  });

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (validName && !create.isPending) create.mutate();
  }

  return (
    <form onSubmit={submit} className="mx-auto max-w-2xl">
      <PageHeader
        title="Create D1 database"
        description="Choose a name for the database. You can add tables and data after it is created."
      />
      <div className="grid gap-6">
        <Input
          label="Name"
          placeholder="Name"
          value={name}
          onChange={(event) => {
            setName(event.target.value);
            create.reset();
          }}
          autoComplete="off"
          description="Use letters, numbers, hyphens, and underscores."
          {...(name && !validName
            ? {
                error:
                  "Start with a letter or number and use only supported characters.",
              }
            : {})}
        />
        {create.error ? (
          <p className="text-kumo-danger" role="alert">
            {create.error instanceof Error
              ? create.error.message
              : "Unable to create the database."}
          </p>
        ) : null}
        <div className="border-kumo-line flex justify-end gap-2 border-t pt-6">
          <Button
            type="button"
            variant="secondary"
            onClick={() => void navigate({ to: "/d1" })}
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
            {create.isPending ? "Creating…" : "Create"}
          </Button>
        </div>
      </div>
    </form>
  );
}
