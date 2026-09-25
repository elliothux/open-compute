import { Popover } from "@cloudflare/kumo/components/popover";
import { Sidebar, useSidebar } from "@cloudflare/kumo/components/sidebar";
import {
  IconActivity,
  IconArrowUpRight,
  IconBrandGithub,
  IconBrightness,
  IconCheck,
  IconChevronRight,
  IconCode,
  IconHome,
  IconLifebuoy,
  IconLogout,
  IconMoon,
  IconPlus,
  IconRefresh,
  IconSelector,
  IconSun,
} from "@tabler/icons-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useRouterState } from "@tanstack/react-router";
import { useAtomValue } from "jotai";
import { useEffect, useState, type ComponentType } from "react";
import { useAuth } from "../features/auth/auth-atoms";
import { detailBreadcrumbAtom } from "../features/navigation/detail-breadcrumb-atom";
import { useTheme } from "../features/theme/theme-atoms";
import { BrandLogo } from "./brand-logo";
import {
  productIcon,
  type CloudflareProductIconProps,
} from "./cloudflare-product-icons";
import { CodeBlock } from "./code-block";
import { openDialog } from "./dialog-manager";
import { SearchInput } from "./search-input";

type NavIcon = ComponentType<CloudflareProductIconProps>;
type NavItem = { to: string; label: string; icon: NavIcon };
type NavGroup = { label: string; items: NavItem[] };

const groups: NavGroup[] = [
  { label: "", items: [{ to: "/", label: "Account home", icon: IconHome }] },
  {
    label: "Compute",
    items: [
      { to: "/workers", label: "Workers", icon: productIcon("Workers") },
      { to: "/observability", label: "Observability", icon: IconActivity },
      {
        to: "/durable-objects",
        label: "Durable Objects",
        icon: productIcon("Durable Objects"),
      },
      { to: "/queues", label: "Queues", icon: productIcon("Queues") },
      { to: "/workflows", label: "Workflows", icon: productIcon("Workflows") },
      {
        to: "/browser-run",
        label: "Browser Run",
        icon: productIcon("Browser Run"),
      },
      {
        to: "/containers",
        label: "Containers",
        icon: productIcon("Containers"),
      },
      { to: "/sandbox", label: "Sandbox", icon: productIcon("Sandbox") },
    ],
  },
  {
    label: "Storage and databases",
    items: [
      { to: "/kv", label: "KV", icon: productIcon("KV") },
      { to: "/d1", label: "D1", icon: productIcon("D1") },
      { to: "/r2", label: "R2", icon: productIcon("R2") },
      { to: "/vectorize", label: "Vectorize", icon: productIcon("Vectorize") },
    ],
  },
  {
    label: "AI",
    items: [
      { to: "/ai-search", label: "AI Search", icon: productIcon("AI Search") },
    ],
  },
  {
    label: "Manage account",
    items: [{ to: "/platform", label: "Platform", icon: IconCode }],
  },
];
const items = groups.flatMap((group) => group.items);

function active(pathname: string, target: string) {
  return (
    pathname === target || (target !== "/" && pathname.startsWith(`${target}/`))
  );
}

/** Search box that expands the collapsed sidebar when focused. */
function SidebarSearch({
  value,
  onChange,
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  const { setOpen } = useSidebar();
  return (
    <div className="mb-3">
      <SearchInput
        aria-label="Search navigation"
        placeholder="Search navigation"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onFocus={() => setOpen(true)}
      />
    </div>
  );
}

export function AppShell({ children }: { children: React.ReactNode }) {
  const { client, instanceId, setInstanceId, clearAuth } = useAuth();
  const queryClient = useQueryClient();
  const { mode, setMode } = useTheme();
  const pathname = useRouterState({
    select: (state) => state.location.pathname,
  });
  const detailBreadcrumb = useAtomValue(detailBreadcrumbAtom);
  const [navigationSearch, setNavigationSearch] = useState("");
  const [instanceMenuOpen, setInstanceMenuOpen] = useState(false);
  const [instanceSearch, setInstanceSearch] = useState("");
  const accounts = useQuery({
    queryKey: ["cloudflare-v4", "accounts"],
    queryFn: async ({ signal }) =>
      (await client!.accounts.list({}, { signal })).result,
    enabled: client !== null,
  });
  const selectedAccount = accounts.data?.find(
    (account) => account.id === instanceId,
  );
  const visibleAccounts = (accounts.data ?? []).filter((account) => {
    const needle = instanceSearch.trim().toLowerCase();
    return (
      !needle ||
      account.name.toLowerCase().includes(needle) ||
      account.id.toLowerCase().includes(needle)
    );
  });
  const current = items.find((item) => active(pathname, item.to));
  const documentTitle = current
    ? `${current.label} | open-compute`
    : "open-compute";

  useEffect(() => {
    document.title = documentTitle;
  }, [documentTitle]);

  function signOut() {
    clearAuth();
    queryClient.clear();
    window.location.replace(`${import.meta.env.BASE_URL}login`);
  }

  function openCreateInstanceDialog() {
    setInstanceMenuOpen(false);
    openDialog({
      title: "Create an instance",
      description:
        "Instances are isolated open-compute deployments, each with its own data directory and authority. Set one up with the ocd CLI, then select it here.",
      size: "lg",
      content: (
        <div className="mt-4 grid gap-4">
          <CodeBlock
            className="ring-kumo-line overflow-auto rounded-lg p-4 text-xs ring"
            code="ocd instance setup --name my-instance --config ./compute.toml --data-dir ./data --yes"
            copy
            language="bash"
          />
          <a
            className="text-kumo-link inline-flex items-center gap-1 text-sm hover:underline"
            href="https://open-compute.dev/docs/get-started/"
            target="_blank"
            rel="noreferrer"
          >
            Instance setup guide
            <IconArrowUpRight size={14} />
          </a>
        </div>
      ),
    });
  }
  const detailPath =
    current && pathname !== current.to
      ? decodeURIComponent(pathname.slice(current.to.length + 1)).split("/")
      : [];
  const createName =
    detailPath[0] === "new"
      ? (
          {
            "/d1": "Create database",
            "/queues": "Create queue",
            "/r2": "Create bucket",
            "/workflows": "Create workflow",
          } as Record<string, string>
        )[current?.to ?? ""]
      : undefined;
  const detailNameFromPath =
    createName ??
    (current?.to === "/ai-search" && detailPath[0] === "namespace"
      ? detailPath[1]
      : current?.to === "/ai-search" && detailPath.length > 1
        ? detailPath[1]
        : detailPath[0]);
  const detailName =
    detailBreadcrumb?.path === pathname
      ? detailBreadcrumb.name
      : detailNameFromPath;
  const r2ObjectKey =
    current?.to === "/r2" &&
    detailPath[1] === "objects" &&
    detailPath.at(-1) === "details"
      ? detailPath.slice(2, -1).join("/")
      : null;
  const search = navigationSearch.trim().toLowerCase();

  return (
    <Sidebar.Provider
      defaultOpen
      collapsible="icon"
      mobileBreakpoint={960}
      className="h-svh overflow-hidden"
    >
      <Sidebar className="bg-kumo-base border-kumo-line h-full border-r">
        <Sidebar.Header className="border-kumo-line border-b px-3 py-3">
          <Popover
            open={instanceMenuOpen}
            onOpenChange={(open) => {
              setInstanceMenuOpen(open);
              if (!open) setInstanceSearch("");
            }}
          >
            <Popover.Trigger
              render={
                <button
                  type="button"
                  aria-label="Switch instance"
                  className="hover:bg-kumo-tint flex h-9 w-full items-center gap-2 rounded-lg px-2 text-left"
                />
              }
            >
              <BrandLogo variant="mark" className="size-6 shrink-0" />
              <span className="min-w-0 flex-1 truncate font-medium group-data-[collapsible=icon]:hidden">
                {selectedAccount?.name ?? instanceId ?? "Select instance"}
              </span>
              <IconSelector
                size={16}
                className="text-kumo-subtle shrink-0 group-data-[collapsible=icon]:hidden"
              />
            </Popover.Trigger>
            <Popover.Content align="start" sideOffset={8} className="w-80 p-0">
              <Popover.Title className="sr-only">Switch instance</Popover.Title>
              <div className="border-kumo-line border-b p-3">
                <SearchInput
                  aria-label="Search instances"
                  placeholder="Search instances"
                  value={instanceSearch}
                  onChange={(event) => setInstanceSearch(event.target.value)}
                  autoFocus
                />
              </div>
              <div
                role="listbox"
                aria-label="Instances"
                className="max-h-72 overflow-y-auto p-2"
              >
                {accounts.isLoading ? (
                  <p className="text-kumo-subtle px-3 py-2 text-sm">
                    Loading instances…
                  </p>
                ) : visibleAccounts.length > 0 ? (
                  visibleAccounts.map((account) => (
                    <button
                      key={account.id}
                      type="button"
                      role="option"
                      aria-selected={account.id === instanceId}
                      className="hover:bg-kumo-tint flex w-full items-start gap-2 rounded-lg px-3 py-2 text-left text-sm"
                      onClick={() => {
                        if (account.id === instanceId) {
                          setInstanceMenuOpen(false);
                          return;
                        }
                        queryClient.clear();
                        setInstanceId(account.id);
                        window.location.assign(import.meta.env.BASE_URL);
                      }}
                    >
                      <span className="min-w-0 flex-1">
                        <span className="block truncate font-medium">
                          {account.name}
                        </span>
                        <code className="text-kumo-subtle block truncate text-xs">
                          {account.id}
                        </code>
                      </span>
                      {account.id === instanceId ? (
                        <span className="flex h-lh items-center">
                          <IconCheck size={16} aria-hidden="true" />
                        </span>
                      ) : null}
                    </button>
                  ))
                ) : (
                  <p className="text-kumo-subtle px-3 py-2 text-sm">
                    No matching instances.
                  </p>
                )}
              </div>
              <div className="border-kumo-line border-t p-2">
                <button
                  type="button"
                  className="hover:bg-kumo-tint flex w-full items-center gap-2 rounded-lg px-3 py-2 text-left text-sm font-medium"
                  onClick={openCreateInstanceDialog}
                >
                  <IconPlus size={16} className="shrink-0" />
                  Create instance
                </button>
              </div>
            </Popover.Content>
          </Popover>
        </Sidebar.Header>
        <Sidebar.Content className="overflow-y-auto">
          <SidebarSearch
            value={navigationSearch}
            onChange={setNavigationSearch}
          />
          <nav aria-label="Primary navigation">
            {groups.map((group) => {
              const matching = group.items.filter((item) =>
                item.label.toLowerCase().includes(search),
              );
              if (matching.length === 0) return null;
              return (
                <Sidebar.Group key={group.label || "home"}>
                  {group.label ? (
                    <Sidebar.GroupLabel>{group.label}</Sidebar.GroupLabel>
                  ) : null}
                  <Sidebar.Menu>
                    {matching.map((item) => (
                      <Sidebar.MenuButton
                        key={item.to}
                        href={item.to}
                        icon={item.icon}
                        active={active(pathname, item.to)}
                        tooltip={item.label}
                        itemId={item.to}
                      >
                        {item.label}
                      </Sidebar.MenuButton>
                    ))}
                  </Sidebar.Menu>
                </Sidebar.Group>
              );
            })}
            {search &&
            !items.some((item) => item.label.toLowerCase().includes(search)) ? (
              <p className="text-kumo-subtle px-5 py-3">No matching pages.</p>
            ) : null}
          </nav>
        </Sidebar.Content>
        <Sidebar.Footer className="border-kumo-line h-auto flex-col items-stretch border-t py-2">
          <Sidebar.Menu>
            <Sidebar.MenuButton
              href="https://github.com/elliothux/open-compute"
              target="_blank"
              rel="noreferrer"
              icon={IconBrandGithub}
              tooltip="GitHub"
            >
              GitHub
            </Sidebar.MenuButton>
            <Sidebar.MenuButton
              href="https://open-compute.dev/docs/"
              target="_blank"
              rel="noreferrer"
              icon={IconLifebuoy}
              tooltip="Documentation"
            >
              Documentation
            </Sidebar.MenuButton>
            <Sidebar.MenuButton
              icon={
                mode === "light"
                  ? IconSun
                  : mode === "dark"
                    ? IconMoon
                    : IconBrightness
              }
              tooltip="Toggle theme"
              onClick={() =>
                setMode(
                  mode === "light"
                    ? "dark"
                    : mode === "dark"
                      ? "system"
                      : "light",
                )
              }
            >
              {mode === "light"
                ? "Light mode"
                : mode === "dark"
                  ? "Dark mode"
                  : "Auto mode"}
            </Sidebar.MenuButton>
            <Sidebar.MenuButton
              className="text-kumo-danger hover:bg-kumo-danger-tint hover:text-kumo-danger"
              icon={IconLogout}
              tooltip="Sign out"
              onClick={signOut}
            >
              Sign out
            </Sidebar.MenuButton>
          </Sidebar.Menu>
        </Sidebar.Footer>
      </Sidebar>

      <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto">
        <header className="border-kumo-line bg-kumo-canvas sticky top-0 z-20 flex h-14 items-center justify-between border-b px-4 sm:px-6">
          <div className="flex min-w-0 items-center gap-2">
            <Sidebar.Trigger aria-label="Toggle navigation" />
            <nav
              className="flex min-w-0 items-center gap-2"
              aria-label="Breadcrumb"
            >
              <Link
                className="text-kumo-subtle hidden shrink-0 hover:underline sm:inline"
                to="/"
              >
                Account home
              </Link>
              {current && current.to !== "/" ? (
                <>
                  <IconChevronRight
                    size={12}
                    className="text-kumo-subtle hidden shrink-0 sm:block"
                  />
                  <Link className="truncate hover:underline" to={current.to}>
                    {current.label}
                  </Link>
                </>
              ) : null}
              {detailName ? (
                <>
                  <IconChevronRight
                    size={12}
                    className="text-kumo-subtle hidden shrink-0 sm:block"
                  />
                  {r2ObjectKey ? (
                    <Link
                      className="hidden max-w-64 truncate hover:underline sm:inline"
                      to="/r2/$bucketId"
                      params={{ bucketId: detailName ?? "" }}
                      search={{ prefix: "" }}
                    >
                      {detailName}
                    </Link>
                  ) : (
                    <span className="hidden max-w-64 truncate font-medium sm:inline">
                      {detailName}
                    </span>
                  )}
                </>
              ) : null}
              {r2ObjectKey ? (
                <>
                  <IconChevronRight
                    size={12}
                    className="text-kumo-subtle hidden shrink-0 sm:block"
                  />
                  <span className="hidden max-w-64 truncate font-medium sm:inline">
                    {r2ObjectKey}
                  </span>
                </>
              ) : null}
            </nav>
          </div>
          <div className="flex items-center gap-1">
            <Link
              to="/platform"
              className="hover:bg-kumo-tint flex h-9 items-center gap-2 rounded-lg px-2.5"
            >
              <IconRefresh size={16} />
              <span className="hidden sm:inline">System status</span>
            </Link>
            <a
              href="https://open-compute.dev"
              target="_blank"
              rel="noreferrer"
              className="hover:bg-kumo-tint flex h-9 items-center gap-2 rounded-lg px-2.5"
            >
              <IconLifebuoy size={16} />
              <span className="hidden sm:inline">Support</span>
            </a>
          </div>
        </header>
        <main className="min-w-0 flex-1 px-4 py-6 sm:px-6 sm:py-8">
          <div className="mx-auto w-full max-w-7xl">{children}</div>
        </main>
      </div>
    </Sidebar.Provider>
  );
}
