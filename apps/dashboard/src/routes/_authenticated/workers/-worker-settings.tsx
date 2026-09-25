import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { Switch } from "@cloudflare/kumo/components/switch";
import { IconPlus, IconTrash } from "@tabler/icons-react";
import type { ReactNode } from "react";
import type { QueueConsumer } from "@open-compute/sdk";
import { ErrorState, LoadingRows } from "../../../components/dashboard-page";
import { WorkerBindingEditor } from "../../../components/worker-binding-editor";
import { WorkerQueueTriggers } from "../../../components/worker-queue-triggers";
import { WorkerVariableEditor } from "../../../components/worker-variable-editor";

type Binding = {
  name?: string;
  type: string;
  text?: string;
  json?: unknown;
  namespace_id?: string;
  bucket_name?: string;
  database_id?: string;
  queue_name?: string;
};
type Secret = { name: string };
type Schedule = { cron: string };

const sections = [
  ["variables", "Variables and secrets"],
  ["bindings", "Bindings"],
  ["observability", "Observability"],
  ["runtime", "Runtime"],
  ["triggers", "Trigger events"],
  ["general", "General"],
  ["danger", "Danger zone"],
] as const;

function Block({
  id,
  title,
  children,
}: {
  id: string;
  title: string;
  children: ReactNode;
}) {
  return (
    <section id={id} className="scroll-mt-6 space-y-3">
      <h2 className="text-base font-semibold">{title}</h2>
      {children}
    </section>
  );
}

function Card({ children }: { children: ReactNode }) {
  return (
    <div className="bg-kumo-base ring-kumo-line overflow-hidden rounded-lg ring">
      {children}
    </div>
  );
}

function SettingRow({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <div className="flex min-h-14 flex-wrap items-center justify-between gap-3 px-4 py-3">
      <span className="font-medium">{label}</span>
      <div className="text-kumo-subtle min-w-0">{children}</div>
    </div>
  );
}

export function WorkerSettings({
  workerId,
  bindings,
  secrets,
  secretsLoading,
  secretsError,
  schedules,
  queueConsumers,
  observabilityEnabled,
  observabilityLoading,
  observabilityPending,
  compatibilityDate,
  compatibilityFlags,
  cpuTimeLimit,
  publicOrigin,
  publicDraft,
  onPublicDraftChange,
  publicOriginPending,
  onSavePublicOrigin,
  onDisablePublicOrigin,
  onAddSecret,
  onDeleteSecret,
  onAddSchedule,
  onDeleteSchedule,
  onToggleObservability,
  onDeleteWorker,
}: {
  workerId: string;
  bindings: readonly Binding[];
  secrets: readonly Secret[];
  secretsLoading: boolean;
  secretsError: unknown;
  schedules: readonly Schedule[];
  queueConsumers: readonly QueueConsumer[];
  observabilityEnabled: boolean;
  observabilityLoading: boolean;
  observabilityPending: boolean;
  compatibilityDate: string | undefined;
  compatibilityFlags: readonly string[] | undefined;
  cpuTimeLimit: number | undefined;
  publicOrigin: string | undefined;
  publicDraft: string;
  onPublicDraftChange: (value: string) => void;
  publicOriginPending: boolean;
  onSavePublicOrigin: () => void;
  onDisablePublicOrigin: () => void;
  onAddSecret: () => void;
  onDeleteSecret: (name: string) => void;
  onAddSchedule: () => void;
  onDeleteSchedule: (cron: string) => void;
  onToggleObservability: (enabled: boolean) => void;
  onDeleteWorker: () => void;
}) {
  return (
    <div className="mx-auto grid max-w-6xl items-start gap-10 xl:grid-cols-4">
      <div className="space-y-10 xl:col-span-3">
        <Block id="variables" title="Runtime variables and secrets">
          {secretsLoading ? (
            <LoadingRows count={1} />
          ) : secretsError ? (
            <ErrorState error={secretsError} />
          ) : (
            <WorkerVariableEditor
              workerId={workerId}
              bindings={bindings}
              secrets={secrets}
              onAddSecret={onAddSecret}
              onDeleteSecret={onDeleteSecret}
            />
          )}
        </Block>
        <Block id="bindings" title="Bindings">
          <WorkerBindingEditor
            workerId={workerId}
            bindings={bindings}
            secretNames={secrets.map((item) => item.name)}
          />
        </Block>
        <Block id="observability" title="Observability">
          <Card>
            <SettingRow label="Workers Logs">
              <Switch
                aria-label="Workers Logs"
                checked={observabilityEnabled}
                disabled={observabilityLoading || observabilityPending}
                onCheckedChange={onToggleObservability}
              />
            </SettingRow>
          </Card>
        </Block>
        <Block id="runtime" title="Runtime">
          <Card>
            <SettingRow label="Compatibility date">
              {compatibilityDate ?? "—"}
            </SettingRow>
          </Card>
          <Card>
            <SettingRow label="Compatibility flags">
              {compatibilityFlags?.length
                ? compatibilityFlags.join(", ")
                : "None"}
            </SettingRow>
          </Card>
        </Block>
        <Block id="triggers" title="Trigger events">
          <Card>
            <div className="border-kumo-line border-b px-4 py-3 font-medium">
              Cron triggers
            </div>
            <div className="space-y-2 px-4 py-3">
              {schedules.length ? (
                schedules.map((item) => (
                  <div
                    key={item.cron}
                    className="bg-kumo-tint flex items-center justify-between rounded-md px-3 py-1.5"
                  >
                    <code className="text-xs">{item.cron}</code>
                    <Button
                      variant="ghost"
                      shape="square"
                      aria-label={`Delete ${item.cron}`}
                      onClick={() => onDeleteSchedule(item.cron)}
                    >
                      <IconTrash size={16} />
                    </Button>
                  </div>
                ))
              ) : (
                <p className="text-kumo-subtle">No cron triggers configured.</p>
              )}
              <div className="flex justify-end">
                <Button variant="secondary" onClick={onAddSchedule}>
                  <IconPlus size={16} /> Add
                </Button>
              </div>
            </div>
          </Card>
          <WorkerQueueTriggers workerId={workerId} consumers={queueConsumers} />
        </Block>
        <Block id="general" title="General">
          <Card>
            <SettingRow label="Name">
              <code className="text-xs">{workerId}</code>
            </SettingRow>
          </Card>
          <Card>
            <SettingRow label="CPU time limit">
              {cpuTimeLimit === undefined ? "—" : `${cpuTimeLimit} ms`}
            </SettingRow>
          </Card>
          <Card>
            <div className="border-kumo-line border-b px-4 py-3 font-medium">
              Public origin
            </div>
            <div className="flex flex-wrap items-end gap-2 px-4 py-3">
              <Input
                className="min-w-48 flex-1"
                label="Public name"
                value={publicDraft}
                onChange={(event) => onPublicDraftChange(event.target.value)}
              />
              <Button
                variant="primary"
                disabled={!publicDraft.trim() || publicOriginPending}
                onClick={onSavePublicOrigin}
              >
                Save
              </Button>
              {publicOrigin ? (
                <Button
                  variant="secondary"
                  disabled={publicOriginPending}
                  onClick={onDisablePublicOrigin}
                >
                  Disable
                </Button>
              ) : null}
            </div>
          </Card>
        </Block>
        <Block id="danger" title="Danger zone">
          <Card>
            <SettingRow label="Delete Worker">
              <Button variant="destructive" onClick={onDeleteWorker}>
                <IconTrash size={16} /> Delete
              </Button>
            </SettingRow>
          </Card>
        </Block>
      </div>
      <nav
        aria-label="Settings sections"
        className="border-kumo-line sticky top-6 hidden border-l pl-4 xl:grid xl:gap-3"
      >
        {sections.map(([id, label]) => (
          <a
            key={id}
            href={`#${id}`}
            className="text-kumo-subtle hover:text-kumo-default text-sm"
          >
            {label}
          </a>
        ))}
      </nav>
    </div>
  );
}
