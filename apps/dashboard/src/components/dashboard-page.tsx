import { Badge } from "@cloudflare/kumo/components/badge";
import { Empty } from "@cloudflare/kumo/components/empty";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { Tabs } from "@cloudflare/kumo/components/tabs";
import { Toolbar } from "@cloudflare/kumo/components/toolbar";
import { IconInbox, IconRefresh } from "@tabler/icons-react";
import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { SearchInput } from "./search-input";

export function PageHeader({
  title,
  description,
  actions,
  extension = false,
}: {
  title: string;
  description?: string;
  actions?: ReactNode;
  extension?: boolean;
}) {
  return (
    <div className="mb-6 flex flex-wrap items-start justify-between gap-4">
      <div className="grid gap-1.5">
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-xl font-semibold">{title}</h1>
          {extension ? <Badge variant="neutral">open-compute</Badge> : null}
        </div>
        {description ? (
          <p className="text-kumo-subtle max-w-3xl text-sm">{description}</p>
        ) : null}
      </div>
      {actions ? (
        <div className="flex flex-wrap items-center gap-2">{actions}</div>
      ) : null}
    </div>
  );
}

export function PageTabs({
  items,
  active,
}: {
  items: readonly { label: string; href: string }[];
  active: string;
}) {
  return (
    <nav className="mb-6" aria-label="Resource tabs">
      <Tabs
        variant="underline"
        value={active}
        tabs={items.map((item) => ({
          value: item.label,
          label: item.label,
          nativeButton: false,
          render: <Link to={item.href} />,
        }))}
      />
    </nav>
  );
}

export function Section({
  title,
  description,
  children,
}: {
  title: string;
  description?: string;
  children: ReactNode;
}) {
  return (
    <section className="grid gap-3">
      <div className="grid gap-1">
        <h2 className="text-base font-semibold">{title}</h2>
        {description ? (
          <p className="text-kumo-subtle text-sm">{description}</p>
        ) : null}
      </div>
      {children}
    </section>
  );
}

export function Panel({
  children,
  className = "",
}: {
  children: ReactNode;
  className?: string;
}) {
  return <LayerCard className={`px-5 py-4 ${className}`}>{children}</LayerCard>;
}

export function StatGrid({
  items,
}: {
  items: readonly { label: string; value: ReactNode; detail?: string }[];
}) {
  return (
    <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
      {items.map((item) => (
        <LayerCard key={item.label} className="grid gap-1 px-4 py-3">
          <span className="text-kumo-subtle text-sm">{item.label}</span>
          <strong className="text-xl font-semibold">{item.value}</strong>
          {item.detail ? (
            <span className="text-kumo-subtle text-xs">{item.detail}</span>
          ) : null}
        </LayerCard>
      ))}
    </div>
  );
}

export function CatalogToolbar({
  value,
  onChange,
  onRefresh,
  refreshing = false,
  placeholder = "Search resources",
}: {
  value: string;
  onChange: (value: string) => void;
  onRefresh: () => void;
  refreshing?: boolean;
  placeholder?: string;
}) {
  return (
    <div className="bg-kumo-recessed ring-kumo-line mb-5 rounded-xl p-1 ring">
      <Toolbar className="w-full">
        <SearchInput
          aria-label={placeholder}
          className="min-w-0 flex-1 rounded-r-none"
          placeholder={placeholder}
          value={value}
          onChange={(event) => onChange(event.target.value)}
        />
        <Toolbar.Button
          aria-label="Refresh"
          onClick={onRefresh}
          disabled={refreshing}
        >
          <IconRefresh size={16} className={refreshing ? "animate-spin" : ""} />
        </Toolbar.Button>
      </Toolbar>
    </div>
  );
}

export function Notice({
  children,
  tone = "info",
}: {
  children: ReactNode;
  tone?: "info" | "danger" | "warning";
}) {
  const style =
    tone === "danger"
      ? "bg-kumo-danger-tint text-kumo-danger"
      : tone === "warning"
        ? "bg-kumo-warning-tint"
        : "bg-kumo-info-tint";
  return (
    <div className={`${style} rounded-lg px-4 py-3 text-sm`}>{children}</div>
  );
}

export function LoadingRows({ count = 4 }: { count?: number }) {
  return (
    <div className="grid gap-3" aria-label="Loading">
      {Array.from({ length: count }, (_, index) => (
        <div
          key={index}
          className="bg-kumo-base ring-kumo-line h-20 animate-pulse rounded-lg ring"
        />
      ))}
    </div>
  );
}

export function EmptyState({
  title,
  description,
  action,
}: {
  title: string;
  description: string;
  action?: ReactNode;
}) {
  return (
    <Empty
      icon={<IconInbox size={32} />}
      title={title}
      description={description}
      contents={action}
      size="sm"
    />
  );
}

export function ErrorState({ error }: { error: unknown }) {
  return (
    <Notice tone="danger">
      {error instanceof Error ? error.message : "Unable to load this page."}
    </Notice>
  );
}

export function ResourceList({ children }: { children: ReactNode }) {
  return <div className="grid gap-3">{children}</div>;
}

export function ResourceRow({
  href,
  icon,
  title,
  description,
  meta,
  footer,
}: {
  href: string;
  icon: ReactNode;
  title: string;
  description?: string;
  meta?: ReactNode;
  footer?: ReactNode;
}) {
  return (
    <LayerCard className="overflow-hidden p-0">
      <Link
        to={href}
        className="hover:bg-kumo-tint flex min-h-16 items-center gap-3 px-4 py-3"
      >
        <span className="text-kumo-brand flex h-lh items-center">{icon}</span>
        <span className="min-w-0 flex-1">
          <span className="block truncate font-medium">{title}</span>
          {description ? (
            <span className="text-kumo-subtle block truncate text-sm">
              {description}
            </span>
          ) : null}
        </span>
        {meta ? (
          <span className="text-kumo-subtle shrink-0 text-sm">{meta}</span>
        ) : null}
      </Link>
      {footer ? (
        <div className="border-kumo-line text-kumo-subtle border-t px-4 py-2 text-sm">
          {footer}
        </div>
      ) : null}
    </LayerCard>
  );
}

export function DefinitionList({
  items,
}: {
  items: readonly { label: string; value: ReactNode }[];
}) {
  return (
    <dl className="divide-kumo-line divide-y">
      {items.map((item) => (
        <div key={item.label} className="grid gap-1 py-3 sm:grid-cols-4">
          <dt className="text-kumo-subtle">{item.label}</dt>
          <dd className="min-w-0 break-words sm:col-span-3">{item.value}</dd>
        </div>
      ))}
    </dl>
  );
}
