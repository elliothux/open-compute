import { useRouterState } from "@tanstack/react-router";
import { useEffect, useMemo, useState } from "react";
import type { Icon } from "@phosphor-icons/react";
import {
  Cloud,
  BookOpen,
  Database,
  Graph,
  HardDrives,
  Moon,
  Package,
  Path,
  SignOut,
  SquaresFour,
  Stack,
  Sun,
  Table,
} from "@phosphor-icons/react";
import { Sidebar } from "@cloudflare/kumo/components/sidebar";
import { BrandLogo } from "./BrandLogo";
import { Breadcrumbs } from "./Breadcrumbs";
import { CommandPalette } from "./CommandPalette";
import { useAuth } from "../features/auth/AuthProvider";
import { useTheme } from "../features/theme/ThemeProvider";

type NavItem = {
  to: string;
  label: string;
  icon: Icon;
};

type NavGroup = {
  id: string;
  label: string;
  items: NavItem[];
};

const navGroups: NavGroup[] = [
  {
    id: "overview",
    label: "Overview",
    items: [{ to: "/", label: "Overview", icon: SquaresFour }],
  },
  {
    id: "compute",
    label: "Compute",
    items: [{ to: "/workers", label: "Workers", icon: Cloud }],
  },
  {
    id: "storage",
    label: "Storage",
    items: [
      { to: "/kv", label: "KV", icon: Database },
      { to: "/d1", label: "D1", icon: Table },
      { to: "/r2", label: "R2", icon: Package },
      { to: "/durable-objects", label: "Durable Objects", icon: Graph },
    ],
  },
  {
    id: "platform",
    label: "Platform services",
    items: [
      { to: "/queues", label: "Queues", icon: Stack },
      { to: "/workflows", label: "Workflows", icon: Path },
      { to: "/platform", label: "Platform", icon: HardDrives },
    ],
  },
];

function isActivePath(pathname: string, target: string) {
  return pathname === target || (target !== "/" && pathname.startsWith(`${target}/`));
}

export function AppShell({ children }: { children: React.ReactNode }) {
  const { accountId, clearAuth } = useAuth();
  const { resolved, toggle } = useTheme();
  const pathname = useRouterState({ select: state => state.location.pathname });
  const [recentPaths, setRecentPaths] = useState<string[]>([]);
  const navItems = useMemo(() => navGroups.flatMap(group => group.items), []);

  useEffect(() => {
    const current = navItems.find(item => isActivePath(pathname, item.to));
    if (!current || current.to === "/") return;
    const storageKey = "open-compute.operator.recent";
    let stored: string[] = [];
    try {
      const value = sessionStorage.getItem(storageKey);
      if (value) stored = JSON.parse(value) as string[];
    } catch {
      stored = [];
    }
    const next = [current.to, ...stored.filter(path => path !== current.to)].slice(0, 4);
    sessionStorage.setItem(storageKey, JSON.stringify(next));
    setRecentPaths(next);
  }, [navItems, pathname]);

  const recentItems = recentPaths
    .map(path => navItems.find(item => item.to === path))
    .filter((item): item is NavItem => item !== undefined);

  return (
    <Sidebar.Provider defaultOpen collapsible="icon" mobileBreakpoint={960} className="min-h-full bg-kumo-base text-kumo-default">
      <Sidebar>
        <Sidebar.Header className="flex flex-col items-start gap-1 border-b border-kumo-line px-4 py-4">
          <BrandLogo variant="wordmark" />
          <span className="text-xs text-kumo-subtle group-data-[collapsible=icon]:hidden">
            Operator dashboard
          </span>
          {accountId ? (
            <code className="max-w-full truncate [font-size:0.9em] text-kumo-subtle group-data-[collapsible=icon]:hidden">
              Account {accountId.slice(0, 8)}…
            </code>
          ) : null}
        </Sidebar.Header>
        <Sidebar.Content>
          <nav aria-label="Primary navigation">
            {navGroups.map(group => (
              <Sidebar.Group key={group.id}>
                <Sidebar.GroupLabel>{group.label}</Sidebar.GroupLabel>
                <Sidebar.Menu>
                  {group.items.map(item => (
                    <Sidebar.MenuButton
                      key={item.to}
                      href={item.to}
                      icon={item.icon}
                      active={isActivePath(pathname, item.to)}
                      tooltip={item.label}
                      itemId={item.to}
                    >
                      {item.label}
                    </Sidebar.MenuButton>
                  ))}
                </Sidebar.Menu>
              </Sidebar.Group>
            ))}
            {recentItems.length > 0 ? (
              <Sidebar.Group>
                <Sidebar.GroupLabel>Recent</Sidebar.GroupLabel>
                <Sidebar.Menu>
                  {recentItems.map(item => (
                    <Sidebar.MenuButton
                      key={`recent-${item.to}`}
                      href={item.to}
                      icon={item.icon}
                      active={isActivePath(pathname, item.to)}
                      tooltip={item.label}
                      itemId={`recent-${item.to}`}
                    >
                      {item.label}
                    </Sidebar.MenuButton>
                  ))}
                </Sidebar.Menu>
              </Sidebar.Group>
            ) : null}
          </nav>
        </Sidebar.Content>
        <Sidebar.Footer className="border-t border-kumo-line">
          <Sidebar.Menu>
            <Sidebar.MenuButton
              icon={BookOpen}
              tooltip="Documentation"
              href="https://open-compute.dev/"
            >
              Documentation
            </Sidebar.MenuButton>
            <Sidebar.MenuButton
              icon={resolved === "dark" ? Sun : Moon}
              tooltip={resolved === "dark" ? "Light mode" : "Dark mode"}
              onClick={toggle}
            >
              {resolved === "dark" ? "Light mode" : "Dark mode"}
            </Sidebar.MenuButton>
            <Sidebar.MenuButton icon={SignOut} tooltip="Sign out" onClick={clearAuth}>
              Sign out
            </Sidebar.MenuButton>
          </Sidebar.Menu>
          <Sidebar.Trigger aria-label="Collapse navigation" />
        </Sidebar.Footer>
      </Sidebar>

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="sticky top-0 z-20 border-b border-kumo-line bg-kumo-base/95 px-4 py-3 backdrop-blur sm:px-6">
          <div className="flex items-center justify-between gap-4">
            <div className="flex min-w-0 items-center gap-3">
              <Sidebar.Trigger aria-label="Toggle navigation" />
              <BrandLogo variant="mark" className="size-7 shrink-0" />
              <div className="min-w-0">
                <div className="truncate text-base font-semibold">Operator dashboard</div>
                <div className="hidden truncate text-sm text-kumo-subtle md:block">
                  Manage the open-compute platform through the Operator API.
                </div>
              </div>
            </div>
            <CommandPalette />
          </div>
        </header>
        <main className="flex-1 overflow-auto px-4 py-5 sm:px-6">
          <Breadcrumbs />
          {children}
        </main>
      </div>
    </Sidebar.Provider>
  );
}
