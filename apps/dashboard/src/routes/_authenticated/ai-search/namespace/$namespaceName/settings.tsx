import { Button } from "@cloudflare/kumo/components/button";
import { IconEdit } from "@tabler/icons-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import {
  ErrorState,
  LoadingRows,
  PageTabs,
} from "../../../../../components/dashboard-page";
import { openConfirmDeleteDialog } from "../../../../../components/resource-dialog";
import { useAuth } from "../../../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../../../features/toast/use-mutation-feedback";

export const Route = createFileRoute(
  "/_authenticated/ai-search/namespace/$namespaceName/settings",
)({
  component: NamespaceSettingsPage,
});

function NamespaceSettingsPage() {
  const { namespaceName } = Route.useParams();
  const { client, instanceId: selectedInstanceId } = useAuth();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [editing, setEditing] = useState(false);
  const [description, setDescription] = useState("");
  const namespace = useQuery({
    queryKey: ["ai-search", selectedInstanceId, "namespace", namespaceName],
    queryFn: ({ signal }) =>
      client!.aiSearch.namespaces.read(
        namespaceName,
        { account_id: selectedInstanceId! },
        { signal },
      ),
    enabled: client !== null && selectedInstanceId !== null,
  });
  const update = useMutation({
    mutationFn: () =>
      client!.aiSearch.namespaces.update(namespaceName, {
        account_id: selectedInstanceId!,
        description: description.trim() || null,
      }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: ["ai-search", selectedInstanceId],
      });
      setEditing(false);
      feedback.success("Namespace description updated.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to update the namespace."),
  });
  function confirmDeleteNamespace() {
    openConfirmDeleteDialog({
      name: namespaceName,
      confirm: async () => {
        try {
          await client!.aiSearch.namespaces.delete(namespaceName, {
            account_id: selectedInstanceId!,
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the namespace.");
          throw error;
        }
        await queryClient.invalidateQueries({
          queryKey: ["ai-search", selectedInstanceId],
        });
        feedback.success("Namespace deleted.");
        await navigate({ to: "/ai-search", search: {} });
      },
    });
  }

  const original = namespace.data?.description ?? "";
  const changed = description.trim() !== original;
  const valid = description.length <= 256;
  const base = `/ai-search/namespace/${encodeURIComponent(namespaceName)}`;

  return (
    <div>
      <PageTabs
        active="Settings"
        items={[
          { label: "Playground", href: `${base}/playground` },
          { label: "Settings", href: `${base}/settings` },
        ]}
      />
      {namespace.isLoading ? (
        <LoadingRows count={1} />
      ) : namespace.error ? (
        <ErrorState error={namespace.error} />
      ) : (
        <section className="max-w-3xl">
          <h1 className="mb-3 text-base font-semibold">General</h1>
          <div className="border-kumo-line overflow-hidden rounded-lg border">
            <div className="flex items-center gap-4 px-4 py-4">
              <span className="w-36 shrink-0 font-medium">Description</span>
              <span className="text-kumo-subtle min-w-0 flex-1">
                {original || "No description"}
              </span>
              <Button
                variant="secondary"
                shape="square"
                aria-label="Edit description"
                onClick={() => {
                  setDescription(original);
                  setEditing(true);
                }}
              >
                <IconEdit size={16} />
              </Button>
            </div>
            {editing ? (
              <form
                className="border-kumo-line grid gap-3 border-t px-4 py-4"
                onSubmit={(event) => {
                  event.preventDefault();
                  if (changed && valid && !update.isPending) update.mutate();
                }}
              >
                <textarea
                  aria-label="Namespace description"
                  placeholder="Optional description for this namespace"
                  className="border-kumo-line bg-kumo-control min-h-24 w-full rounded-lg border px-3 py-2 outline-none"
                  maxLength={256}
                  value={description}
                  onChange={(event) => setDescription(event.target.value)}
                  autoFocus
                />
                <div className="flex justify-end gap-2">
                  <Button
                    type="button"
                    variant="secondary"
                    onClick={() => setEditing(false)}
                  >
                    Cancel
                  </Button>
                  <Button
                    type="submit"
                    variant="primary"
                    disabled={!changed || !valid || update.isPending}
                  >
                    Save
                  </Button>
                </div>
              </form>
            ) : null}
            <div className="border-kumo-line flex items-center justify-between gap-4 border-t px-4 py-3">
              <span className="text-kumo-subtle">
                {namespaceName === "default"
                  ? "The default namespace cannot be deleted."
                  : "Deleting a namespace removes its AI Search instances."}
              </span>
              <Button
                variant="destructive"
                disabled={namespaceName === "default"}
                onClick={() => confirmDeleteNamespace()}
              >
                Delete namespace
              </Button>
            </div>
          </div>
        </section>
      )}
    </div>
  );
}
