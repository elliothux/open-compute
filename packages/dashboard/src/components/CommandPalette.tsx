import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import { MagnifyingGlass } from "@phosphor-icons/react";
import { Button } from "@cloudflare/kumo/components/button";
import { CommandPalette as KumoCommandPalette } from "@cloudflare/kumo/components/command-palette";

const destinations = [
  { label: "Overview", to: "/" as const, keywords: "home dashboard" },
  { label: "Workers", to: "/workers" as const, keywords: "compute worker" },
  { label: "KV", to: "/kv" as const, keywords: "storage key value namespace" },
  { label: "D1", to: "/d1" as const, keywords: "database sql sqlite" },
  { label: "R2", to: "/r2" as const, keywords: "object storage bucket s3" },
  { label: "Durable Objects", to: "/durable-objects" as const, keywords: "do namespace object" },
  { label: "Queues", to: "/queues" as const, keywords: "message queue consumer" },
  { label: "Workflows", to: "/workflows" as const, keywords: "workflow definition instance" },
  { label: "Platform", to: "/platform" as const, keywords: "scheduler cache cron images" },
];

type Destination = (typeof destinations)[number];

export function CommandPalette() {
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setOpen(current => !current);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const matches = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return destinations;
    return destinations.filter(item =>
      `${item.label} ${item.keywords}`.toLowerCase().includes(needle),
    );
  }, [query]);

  const close = () => {
    setOpen(false);
    setQuery("");
  };

  const select = (item: Destination) => {
    void navigate({ to: item.to });
    close();
  };

  return (
    <>
      <Button variant="secondary" className="items-center gap-2" onClick={() => setOpen(true)} aria-label="Search pages">
        <MagnifyingGlass size={16} />
        <span className="hidden sm:inline">Search</span>
        <span className="hidden rounded ring ring-kumo-line px-1.5 py-0.5 text-xs text-kumo-subtle md:inline">⌘K</span>
      </Button>
      <KumoCommandPalette.Root
        open={open}
        onOpenChange={nextOpen => {
          if (nextOpen) setOpen(true);
          else close();
        }}
        items={matches}
        value={query}
        onValueChange={setQuery}
        itemToStringValue={item => item.label}
        filter={() => true}
        onSelect={item => select(item)}
      >
        <KumoCommandPalette.Input autoFocus placeholder="Jump to Workers, KV, Platform…" />
        <KumoCommandPalette.List>
          {matches.map(item => (
            <KumoCommandPalette.ResultItem
              key={item.to}
              title={item.label}
              description={item.keywords}
              value={item}
              onClick={() => select(item)}
            />
          ))}
          <KumoCommandPalette.Empty>No matching pages.</KumoCommandPalette.Empty>
        </KumoCommandPalette.List>
        <KumoCommandPalette.Footer>
          Use ↑ and ↓ to move, then Enter to open.
        </KumoCommandPalette.Footer>
      </KumoCommandPalette.Root>
    </>
  );
}
