import { Sidebar } from "@cloudflare/kumo/components/sidebar";
import {
  BookOpen,
  Cloud,
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
  type Icon,
} from "@phosphor-icons/react";
import { useRouterState } from "@tanstack/react-router";
import { useAtomValue, useSetAtom } from "jotai";
import { useEffect, useMemo } from "react";
import { useAuth } from "../features/auth/auth-atoms";
import {
  recentPathsAtom,
  recordRecentPathAtom,
} from "../features/navigation/recent-paths-atoms";
import { useTheme } from "../features/theme/theme-atoms";
import { BrandLogo } from "./brand-logo";
import { Breadcrumbs } from "./breadcrumbs";
import { CommandPalette } from "./command-palette";

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
  return (
    pathname === target || (target !== "/" && pathname.startsWith(`${target}/`))
  );
}

export function AppShell({ children }: { children: React.ReactNode }) {
  const { accountId, clearAuth } = useAuth();
  const { resolved, toggle } = useTheme();
  const pathname = useRouterState({
    select: (state) => state.location.pathname,
  });
  const recentPaths = useAtomValue(recentPathsAtom);
  const recordRecentPath = useSetAtom(recordRecentPathAtom);
  const navItems = useMemo(() => navGroups.flatMap((group) => group.items), []);

  useEffect(() => {
    const current = navItems.find((item) => isActivePath(pathname, item.to));
    if (!current || current.to === "/") return;
    recordRecentPath(current.to);
  }, [navItems, pathname, recordRecentPath]);

  const recentItems = recentPaths
    .map((path) => navItems.find((item) => item.to === path))
    .filter((item): item is NavItem => item !== undefined);

  return (
    <Sidebar.Provider
      defaultOpen
      collapsible="icon"
      mobileBreakpoint={960}
      className="bg-kumo-base text-kumo-default min-h-full"
    >
      <Sidebar>
        <Sidebar.Header className="border-kumo-line flex flex-col items-start gap-1 border-b px-4 py-4">
          <BrandLogo variant="wordmark" />
          <span className="text-kumo-subtle text-xs group-data-[collapsible=icon]:hidden">
            Operator dashboard
          </span>
          {accountId ? (
            <code className="text-kumo-subtle max-w-full truncate [font-size:0.9em] group-data-[collapsible=icon]:hidden">
              Account {accountId.slice(0, 8)}…
            </code>
          ) : null}
        </Sidebar.Header>
        <Sidebar.Content>
          <nav aria-label="Primary navigation">
            {navGroups.map((group) => (
              <Sidebar.Group key={group.id}>
                <Sidebar.GroupLabel>{group.label}</Sidebar.GroupLabel>
                <Sidebar.Menu>
                  {group.items.map((item) => (
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
                  {recentItems.map((item) => (
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
        <Sidebar.Footer className="border-kumo-line border-t">
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
            <Sidebar.MenuButton
              icon={SignOut}
              tooltip="Sign out"
              onClick={clearAuth}
            >
              Sign out
            </Sidebar.MenuButton>
          </Sidebar.Menu>
          <Sidebar.Trigger aria-label="Collapse navigation" />
        </Sidebar.Footer>
      </Sidebar>

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="border-kumo-line bg-kumo-base/95 sticky top-0 z-20 border-b px-4 py-3 backdrop-blur sm:px-6">
          <div className="flex items-center justify-between gap-4">
            <div className="flex min-w-0 items-center gap-3">
              <Sidebar.Trigger aria-label="Toggle navigation" />
              <BrandLogo variant="mark" className="size-7 shrink-0" />
              <div className="min-w-0">
                <div className="truncate text-base font-semibold">
                  Operator dashboard
                </div>
                <div className="text-kumo-subtle hidden truncate text-sm md:block">
                  Manage open-compute through the Cloudflare v4 API.
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
