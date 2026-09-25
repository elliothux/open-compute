import { Button } from "@cloudflare/kumo/components/button";
import { useMutation, useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { useState } from "react";
import type { UpgradeCheck } from "@open-compute/sdk";
import {
  DefinitionList,
  ErrorState,
  LoadingRows,
  Notice,
  PageHeader,
  Panel,
  Section,
  StatGrid,
} from "../../../components/dashboard-page";
import { useAuth } from "../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";

export const Route = createFileRoute("/_authenticated/platform/")({
  component: PlatformPage,
});

function PlatformPage() {
  const { client, instanceId } = useAuth();
  const feedback = useMutationFeedback();
  const [upgradeCheck, setUpgradeCheck] = useState<UpgradeCheck | null>(null);

  const status = useQuery({
    queryKey: ["platform", instanceId],
    queryFn: async ({ signal }) =>
      Promise.all([
        client!.openCompute.capabilities.getForAccount(instanceId!, { signal }),
        client!.openCompute.system.statusForAccount(instanceId!, { signal }),
        client!.openCompute.scheduler.getForAccount(instanceId!, { signal }),
        client!.openCompute.cache.getForAccount(instanceId!, { signal }),
        client!.openCompute.images.capacityForAccount(instanceId!, { signal }),
      ]),
    enabled: client !== null && instanceId !== null,
  });

  const schedulerMutation = useMutation({
    mutationFn: (action: "pause" | "resume" | "repair") => {
      const scheduler = client!.openCompute.scheduler;
      if (action === "pause") return scheduler.pauseForAccount(instanceId!);
      if (action === "resume") return scheduler.resumeForAccount(instanceId!);
      return scheduler.repairForAccount(instanceId!);
    },
    onSuccess: async (_result, action) => {
      await status.refetch();
      feedback.success(`Scheduler ${action} completed.`);
    },
    onError: (error) =>
      feedback.failure(error, "Unable to update the scheduler."),
  });
  const cacheGcMutation = useMutation({
    mutationFn: () =>
      client!.openCompute.cache.collectGarbageForAccount(instanceId!),
    onSuccess: async () => {
      await status.refetch();
      feedback.success("Cache garbage collection completed.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to collect cache garbage."),
  });
  const checkMutation = useMutation({
    mutationFn: () => client!.openCompute.upgrade.checkForAccount(instanceId!),
    onSuccess: (result) => {
      setUpgradeCheck(result);
      feedback.success(
        result.update_available
          ? `Update available: ${result.available_version}. Run ocd upgrade on the host.`
          : "No update available.",
      );
    },
    onError: (error) => feedback.failure(error, "Unable to check for updates."),
  });

  const capabilities = status.data?.[0];
  const system = status.data?.[1];
  const scheduler = status.data?.[2];
  const cache = status.data?.[3];
  const images = status.data?.[4];
  const upgradeCommand = upgradeCheck?.available_version
    ? `ocd upgrade ${upgradeCheck.available_version}`
    : "ocd upgrade";

  return (
    <div className="grid gap-8">
      <PageHeader
        title="Platform"
        description="Installation-scoped open-compute extension status and maintenance."
      />
      {status.isLoading ? (
        <LoadingRows count={4} />
      ) : status.error ? (
        <ErrorState error={status.error} />
      ) : (
        <>
          <StatGrid
            items={[
              {
                label: "System",
                value: system?.state ?? "Unknown",
                detail: system?.version ?? "Version unknown",
              },
              {
                label: "Scheduler",
                value: scheduler?.state ?? "Unknown",
                detail: `${scheduler?.pending ?? 0} pending`,
              },
              {
                label: "Artifact cache",
                value: cache?.entries ?? 0,
                detail: `${cache?.bytes ?? 0} bytes`,
              },
              {
                label: "Images",
                value: `${images?.running ?? 0}/${images?.capacity ?? 0}`,
                detail: `${images?.queued ?? 0} queued`,
              },
            ]}
          />
          {system?.state !== "healthy" ? (
            <Notice tone="warning">
              The installation reports {system?.state ?? "an unknown state"}.
              Review component status before deploying resources.
            </Notice>
          ) : null}
          <Section
            title="Runtime status"
            description="Current component and compatibility information."
          >
            <Panel>
              <DefinitionList
                items={[
                  {
                    label: "Release",
                    value:
                      capabilities?.release ?? system?.version ?? "Unknown",
                  },
                  {
                    label: "Wrangler contract",
                    value: capabilities?.wrangler_version ?? "Unknown",
                  },
                  {
                    label: "Compatibility dates",
                    value: capabilities
                      ? `${capabilities.compatibility_date.minimum} – ${capabilities.compatibility_date.maximum}`
                      : "Unknown",
                  },
                  {
                    label: "Dispatchable deployments",
                    value:
                      system?.deployment_runtime?.dispatchable ?? "Unknown",
                  },
                  {
                    label: "Quarantined deployments",
                    value: system?.deployment_runtime?.quarantined ?? "Unknown",
                  },
                  {
                    label: "Operator proxy",
                    value: system?.operator_proxy.mode ?? "Unknown",
                  },
                ]}
              />
            </Panel>
          </Section>
        </>
      )}
      <Section
        title="Maintenance"
        description="Run installation-level operations. These actions do not modify Cloudflare-compatible resources."
      >
        <Panel className="flex flex-wrap gap-2">
          <Button
            variant="secondary"
            disabled={
              schedulerMutation.isPending || scheduler?.state === "paused"
            }
            onClick={() => schedulerMutation.mutate("pause")}
          >
            Pause scheduler
          </Button>
          <Button
            variant="secondary"
            disabled={
              schedulerMutation.isPending || scheduler?.state !== "paused"
            }
            onClick={() => schedulerMutation.mutate("resume")}
          >
            Resume scheduler
          </Button>
          <Button
            variant="secondary"
            disabled={schedulerMutation.isPending}
            onClick={() => schedulerMutation.mutate("repair")}
          >
            Repair scheduler
          </Button>
          <Button
            variant="secondary"
            disabled={cacheGcMutation.isPending}
            onClick={() => cacheGcMutation.mutate()}
          >
            Collect cache garbage
          </Button>
          <Button
            variant="secondary"
            disabled={checkMutation.isPending}
            onClick={() => checkMutation.mutate()}
          >
            Check for updates
          </Button>
        </Panel>
        {upgradeCheck ? (
          <Notice tone={upgradeCheck.update_available ? "info" : "warning"}>
            {upgradeCheck.update_available
              ? `Version ${upgradeCheck.available_version ?? "unknown"} is available. Run ${upgradeCommand} on the host.`
              : `Version ${upgradeCheck.current_version} is up to date.`}
            {upgradeCheck.blocked_reason
              ? ` ${upgradeCheck.blocked_reason}`
              : ""}
          </Notice>
        ) : null}
      </Section>
    </div>
  );
}
