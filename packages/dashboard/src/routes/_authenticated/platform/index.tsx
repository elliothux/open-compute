import { useMutation, useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { Button } from "@cloudflare/kumo/components/button";
import { DataTable, ErrorState, LoadingState, PageHeader } from "../../../components/PageLayout";
import { useAuth } from "../../../features/auth/AuthProvider";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";

export const Route = createFileRoute("/_authenticated/platform/")({ component: PlatformPage });

function PlatformPage() {
  const { client } = useAuth();
  const feedback = useMutationFeedback();
  const status = useQuery({
    queryKey: ["cloudflare-v4", "open-compute", "platform"],
    queryFn: async ({ signal }) => Promise.all([
      client!.openCompute.scheduler.get({ signal }),
      client!.openCompute.cache.get({ signal }),
      client!.openCompute.images.capacity({ signal }),
    ]),
    enabled: client !== null,
  });
  const schedulerMutation = useMutation({
    mutationFn: (action: "pause" | "resume" | "repair") => client!.openCompute.scheduler[action](),
    onSuccess: async (_result, action) => {
      await status.refetch();
      feedback.success(`Scheduler ${action} completed.`);
    },
    onError: error => feedback.failure(error, "Unable to update the scheduler."),
  });
  const cacheGcMutation = useMutation({
    mutationFn: () => client!.openCompute.cache.collectGarbage(),
    onSuccess: async () => {
      await status.refetch();
      feedback.success("Cache garbage collection completed.");
    },
    onError: error => feedback.failure(error, "Unable to collect cache garbage."),
  });
  const scheduler = status.data?.[0];
  return <div><PageHeader title="Platform" description="Installation-scoped open-compute extension status and maintenance." />
    <div className="mb-4 flex flex-wrap gap-2">
      <Button variant="secondary" disabled={schedulerMutation.isPending || scheduler?.state === "paused"} onClick={() => schedulerMutation.mutate("pause")}>Pause scheduler</Button>
      <Button variant="secondary" disabled={schedulerMutation.isPending || scheduler?.state !== "paused"} onClick={() => schedulerMutation.mutate("resume")}>Resume scheduler</Button>
      <Button variant="secondary" disabled={schedulerMutation.isPending} onClick={() => schedulerMutation.mutate("repair")}>Repair scheduler</Button>
      <Button variant="secondary" disabled={cacheGcMutation.isPending} onClick={() => cacheGcMutation.mutate()}>Collect cache garbage</Button>
    </div>
    {status.isLoading ? <LoadingState /> : status.error ? <ErrorState message="Unable to load platform status." /> : (
      <DataTable columns={[{ key: "name", label: "Component" }, { key: "detail", label: "Status" }]} rows={[
        { name: "Scheduler", detail: `${scheduler?.state ?? "unknown"}; ${scheduler?.pending ?? 0} pending; ${scheduler?.running ?? 0} running` },
        { name: "Cache", detail: `${status.data?.[1].entries ?? 0} entries; ${status.data?.[1].bytes ?? 0} bytes` },
        { name: "Images", detail: `${status.data?.[2].running ?? 0}/${status.data?.[2].capacity ?? 0} running; ${status.data?.[2].queued ?? 0} queued` },
      ]} />
    )}
  </div>;
}
