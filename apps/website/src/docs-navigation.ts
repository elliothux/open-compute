import type { StarlightRouteData } from "@astrojs/starlight/route-data";

type Language = "en" | "zh";
type Label = Record<Language, string>;
type NavigationNode =
  | { label: Label; route: string }
  | { collapsed?: boolean; items: NavigationNode[]; label: Label };

const labels = (en: string, zh: string): Label => ({ en, zh });

const product = (name: string, route: string): NavigationNode => ({
  collapsed: true,
  label: labels(name, name),
  items: [
    { label: labels("Overview", "概述"), route },
    { label: labels("Get started", "上手"), route: `${route}/get-started` },
    { label: labels("Concepts", "概念"), route: `${route}/concepts` },
    { label: labels("Guides", "指南"), route: `${route}/guides` },
    { label: labels("Examples", "示例"), route: `${route}/examples` },
    { label: labels("Limits", "限制"), route: `${route}/platform/limits` },
    {
      label: labels("Behavior differences", "行为差异"),
      route: `${route}/platform/deviations`,
    },
  ],
});

const navigation: NavigationNode[] = [
  {
    label: labels("Start", "开始"),
    items: [
      { label: labels("Overview", "概述"), route: "" },
      { label: labels("Get started", "上手"), route: "/get-started" },
      { label: labels("Directory", "产品目录"), route: "/directory" },
    ],
  },
  {
    label: labels("Workers", "Workers"),
    items: [
      { label: labels("Overview", "概述"), route: "/workers" },
      {
        label: labels("Get started", "上手"),
        route: "/workers/get-started",
      },
      { label: labels("Concepts", "概念"), route: "/workers/concepts" },
      { label: labels("Examples", "示例"), route: "/workers/examples" },
      {
        label: labels("Projects and targets", "项目与部署目标"),
        route: "/workers/projects",
      },
      {
        collapsed: true,
        label: labels("Configuration", "配置"),
        items: [
          {
            label: labels("Overview", "概述"),
            route: "/workers/configuration",
          },
          {
            label: labels("Bindings", "绑定"),
            route: "/workers/configuration/bindings",
          },
          {
            label: labels("Compatibility dates", "兼容日期"),
            route: "/workers/configuration/compatibility-dates",
          },
          {
            label: labels("Compatibility flags", "兼容标志"),
            route: "/workers/configuration/compatibility-flags",
          },
          {
            label: labels("Cron Triggers", "Cron 触发器"),
            route: "/workers/configuration/cron-triggers",
          },
          {
            label: labels("Environment variables", "环境变量"),
            route: "/workers/configuration/environment-variables",
          },
          {
            label: labels("Secrets", "密钥"),
            route: "/workers/configuration/secrets",
          },
          {
            label: labels("Routing", "路由"),
            route: "/workers/configuration/routing",
          },
        ],
      },
      {
        label: labels("Versions and deployments", "版本与部署"),
        route: "/workers/versions-and-deployments",
      },
      {
        label: labels("Static Assets", "静态资源"),
        route: "/workers/static-assets",
      },
      { label: labels("Cache", "缓存"), route: "/workers/cache" },
      {
        collapsed: true,
        label: labels("Runtime APIs", "运行时 API"),
        items: [
          {
            label: labels("Overview", "概述"),
            route: "/workers/runtime-apis",
          },
          {
            label: labels("Handlers", "Handlers"),
            route: "/workers/runtime-apis/handlers",
          },
          {
            label: labels("Bindings", "绑定"),
            route: "/workers/runtime-apis/bindings",
          },
          {
            label: labels("Cache", "缓存"),
            route: "/workers/runtime-apis/cache",
          },
          {
            label: labels("WebSockets", "WebSockets"),
            route: "/workers/runtime-apis/websockets",
          },
          {
            label: labels("TCP sockets", "TCP sockets"),
            route: "/workers/runtime-apis/tcp-sockets",
          },
          {
            label: labels("Node.js compatibility", "Node.js 兼容"),
            route: "/workers/runtime-apis/nodejs",
          },
        ],
      },
      {
        collapsed: true,
        label: labels("Platform", "平台"),
        items: [
          {
            label: labels("Limits", "限制"),
            route: "/workers/platform/limits",
          },
          {
            label: labels("Known issues", "已知问题"),
            route: "/workers/platform/known-issues",
          },
          {
            label: labels("Changelog", "更新日志"),
            route: "/workers/platform/changelog",
          },
        ],
      },
    ],
  },
  {
    label: labels("Storage", "存储"),
    items: [
      product("KV", "/kv"),
      product("D1", "/d1"),
      product("R2", "/r2"),
      product("Vectorize", "/vectorize"),
    ],
  },
  {
    label: labels("Compute", "计算"),
    items: [
      {
        ...product("Durable Objects", "/durable-objects"),
        items: [
          ...(
            product("Durable Objects", "/durable-objects") as {
              items: NavigationNode[];
            }
          ).items,
          {
            label: labels("Alarms", "Alarms"),
            route: "/durable-objects/alarms",
          },
        ],
      },
      product("Queues", "/queues"),
      product("Workflows", "/workflows"),
    ],
  },
  {
    label: labels("Media", "媒体"),
    items: [product("Images", "/images")],
  },
  {
    label: labels("AI", "AI"),
    items: [product("AI Search", "/ai-search")],
  },
  {
    label: labels("Platform", "平台"),
    items: [
      { label: labels("Overview", "概述"), route: "/platform" },
      {
        label: labels("Compatibility", "兼容性"),
        route: "/platform/compatibility",
      },
      {
        label: labels("Behavior differences", "行为差异"),
        route: "/platform/deviations",
      },
      { label: labels("Limits", "限制"), route: "/platform/limits" },
      {
        label: labels("Unsupported", "不支持"),
        route: "/platform/unsupported",
      },
      {
        label: labels("API reference", "API 参考"),
        route: "/platform/reference/api",
      },
    ],
  },
  {
    label: labels("Operate ocd", "运维 ocd"),
    items: [
      { label: labels("Overview", "概述"), route: "/ocd" },
      {
        label: labels("Install and first start", "安装与首次启动"),
        route: "/ocd/get-started",
      },
      { label: labels("Configuration", "配置"), route: "/ocd/configuration" },
      { label: labels("Deploy", "部署"), route: "/ocd/deploy" },
      { label: labels("Health checks", "健康检查"), route: "/ocd/health" },
      {
        label: labels("Backup and retention", "备份与保留"),
        route: "/ocd/backup",
      },
      { label: labels("CLI reference", "常用命令"), route: "/ocd/cli" },
      {
        label: labels("Incident handbook", "故障手册"),
        route: "/ocd/incidents",
      },
    ],
  },
];

const normalizePath = (path: string) => path.replace(/\/+$/, "") || "/";

export function createDocsSidebar(
  language: Language,
  pathname: string,
): StarlightRouteData["sidebar"] {
  const prefix = language === "zh" ? "/docs/zh" : "/docs";
  const currentPath = normalizePath(pathname);

  const convert = (
    node: NavigationNode,
  ): StarlightRouteData["sidebar"][number] => {
    if ("route" in node) {
      const href = `${prefix}${node.route}/`.replace(/\/{2,}/g, "/");
      return {
        type: "link",
        label: node.label[language],
        href,
        isCurrent: normalizePath(href) === currentPath,
        badge: undefined,
        attrs: {},
      };
    }

    return {
      type: "group",
      label: node.label[language],
      entries: node.items.map(convert),
      collapsed: node.collapsed ?? false,
      badge: undefined,
    };
  };

  return navigation.map(convert);
}

export function flattenDocsSidebar(
  sidebar: StarlightRouteData["sidebar"],
): Extract<StarlightRouteData["sidebar"][number], { type: "link" }>[] {
  return sidebar.flatMap((entry) =>
    entry.type === "link" ? [entry] : flattenDocsSidebar(entry.entries),
  );
}
