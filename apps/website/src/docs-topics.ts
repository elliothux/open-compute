import type { StarlightRouteData } from "@astrojs/starlight/route-data";
import type {
  StarlightSidebarTopicsUserConfig,
  StarlightSidebarTopicsUserOptions,
} from "starlight-sidebar-topics";

export type DocsLanguage = "en" | "zh";

type Label = Record<DocsLanguage, string>;
type NavigationNode =
  | { label: Label; route: string }
  | {
      collapsed?: boolean;
      icon: string;
      items: NavigationNode[];
      label: Label;
    };
type TopicScope = { nested?: boolean; prefix: string };
type TopicDefinition = {
  icon: string;
  id: string;
  label: Label;
  link: string;
  items: NavigationNode[];
  scope: TopicScope[];
};
type TopicSidebarItem =
  | { label: string; slug: string }
  | {
      collapsed: boolean;
      items: TopicSidebarItem[];
      label: string;
    };

const languages: DocsLanguage[] = ["en", "zh"];
const localeGroupLabels: Record<DocsLanguage, string> = {
  en: "__open_compute_en",
  zh: "__open_compute_zh",
};

const topicIcons = {
  operate: "i-tabler:activity-heartbeat",
  products: "i-tabler:packages",
  reference: "i-tabler:braces",
  start: "i-tabler:rocket",
} as const;

const groupIcons = {
  artifacts: "i-tabler:archive",
  cli: "i-tabler:terminal-2",
  compute: "i-tabler:cpu",
  develop: "i-tabler:code",
  mediaAi: "i-tabler:sparkles",
  operate: "i-tabler:server-cog",
  overview: "i-tabler:layout-grid",
  project: "i-tabler:git-branch",
  reference: "i-tabler:book-2",
  start: "i-tabler:flag",
  storage: "i-tabler:database",
} as const;

export const docsNavigationIconClasses = [
  ...Object.values(topicIcons),
  ...Object.values(groupIcons),
];

const labels = (en: string, zh: string): Label => ({ en, zh });
const link = (en: string, zh: string, route: string): NavigationNode => ({
  label: labels(en, zh),
  route,
});
const group = (
  en: string,
  zh: string,
  icon: string,
  items: NavigationNode[],
  collapsed = false,
): NavigationNode => ({ collapsed, icon, items, label: labels(en, zh) });

const topics: TopicDefinition[] = [
  {
    icon: topicIcons.start,
    id: "start",
    label: labels("Start", "开始"),
    link: "",
    items: [
      group("Start", "开始", groupIcons.start, [
        link("Documentation", "文档首页", ""),
        link("Get started", "快速开始", "/get-started"),
      ]),
      group("Develop", "开发应用", groupIcons.develop, [
        link("Application workflow", "应用工作流", "/develop"),
        link("Workers", "Workers", "/workers"),
        link("Python example", "Python 示例", "/workers/languages/python"),
        link("Rust example", "Rust 示例", "/workers/languages/rust"),
        link("Configuration", "项目配置", "/workers/configuration"),
        link("Bindings", "Bindings", "/workers/configuration/bindings"),
        link(
          "Versions and rollback",
          "版本与回滚",
          "/workers/versions-and-deployments",
        ),
        link("Static assets", "静态资源", "/workers/static-assets"),
        link("Cache", "缓存", "/workers/cache"),
        link("Runtime APIs", "运行时 API", "/workers/runtime-apis"),
      ]),
    ],
    scope: [
      { prefix: "" },
      { prefix: "/get-started" },
      { prefix: "/develop" },
      { nested: true, prefix: "/workers" },
    ],
  },
  {
    icon: topicIcons.operate,
    id: "operate",
    label: labels("Operate", "运维"),
    link: "/operate",
    items: [
      group("Operate", "运行与运维", groupIcons.operate, [
        link("Operator guide", "运维指南", "/operate"),
        link("Configuration", "平台配置", "/ocd/configuration"),
        link("Health and monitoring", "健康与监控", "/ocd/health"),
        link("Backup and retention", "备份与保留", "/ocd/backup"),
        link("Incident handbook", "故障手册", "/ocd/incidents"),
      ]),
      group("CLI", "CLI", groupIcons.cli, [
        link("Command guide", "命令指南", "/cli"),
      ]),
    ],
    scope: [
      { prefix: "/operate" },
      { prefix: "/cli" },
      { nested: true, prefix: "/ocd" },
    ],
  },
  {
    icon: topicIcons.products,
    id: "products",
    label: labels("Products", "产品"),
    link: "/products",
    items: [
      group("Overview", "概览", groupIcons.overview, [
        link("Product overview", "产品概览", "/products"),
      ]),
      group("Storage", "存储", groupIcons.storage, [
        link("KV", "KV", "/kv"),
        link("D1", "D1", "/d1"),
        link("R2", "R2", "/r2"),
        link("Vectorize", "Vectorize", "/vectorize"),
      ]),
      group("Compute", "计算", groupIcons.compute, [
        link("Durable Objects", "Durable Objects", "/durable-objects"),
        link("Queues", "Queues", "/queues"),
        link("Workflows", "Workflows", "/workflows"),
      ]),
      group("Media and AI", "媒体与 AI", groupIcons.mediaAi, [
        link("Images", "Images", "/images"),
        link("AI Search", "AI Search", "/ai-search"),
      ]),
      group("Artifacts", "Artifacts", groupIcons.artifacts, [
        link("Artifact storage", "制品存储", "/artifacts"),
      ]),
    ],
    scope: [
      { prefix: "/products" },
      { nested: true, prefix: "/kv" },
      { nested: true, prefix: "/d1" },
      { nested: true, prefix: "/r2" },
      { nested: true, prefix: "/vectorize" },
      { nested: true, prefix: "/durable-objects" },
      { nested: true, prefix: "/queues" },
      { nested: true, prefix: "/workflows" },
      { nested: true, prefix: "/images" },
      { nested: true, prefix: "/ai-search" },
      { nested: true, prefix: "/artifacts" },
    ],
  },
  {
    icon: topicIcons.reference,
    id: "reference",
    label: labels("Reference", "参考"),
    link: "/reference",
    items: [
      group("Reference", "参考", groupIcons.reference, [
        link("Reference overview", "参考概览", "/reference"),
        link("Compatibility", "兼容性", "/platform/compatibility"),
        link("Behavior differences", "行为差异", "/platform/deviations"),
        link("Limits", "限制", "/platform/limits"),
        link("Not available", "未提供", "/platform/unsupported"),
        link("Worker API index", "Worker API 索引", "/platform/reference/api"),
      ]),
      group("Project", "项目", groupIcons.project, [
        link("Architecture and contributing", "架构与贡献", "/project"),
      ]),
    ],
    scope: [
      { prefix: "/reference" },
      { nested: true, prefix: "/platform" },
      { nested: true, prefix: "/project" },
    ],
  },
];

function docsSlug(route: string, language: DocsLanguage): string {
  const prefix = language === "zh" ? "docs/zh" : "docs";
  return `${prefix}${route}`;
}

function docsHref(route: string, language: DocsLanguage): string {
  return `/${docsSlug(route, language)}/`.replace(/\/{2,}/g, "/");
}

function toSidebarItem(
  node: NavigationNode,
  language: DocsLanguage,
): TopicSidebarItem {
  if ("route" in node) {
    return {
      label: node.label[language],
      slug: docsSlug(node.route, language),
    };
  }

  return {
    collapsed: node.collapsed ?? false,
    items: node.items.map((item) => toSidebarItem(item, language)),
    label: node.label[language],
  };
}

export const docsSidebarTopics = topics.map((topic) => ({
  icon: topic.icon,
  id: topic.id,
  items: languages.map((language) => ({
    label: localeGroupLabels[language],
    items: topic.items.map((item) => toSidebarItem(item, language)),
  })),
  label: topic.label.en,
  link: docsHref(topic.link, "en"),
})) satisfies StarlightSidebarTopicsUserConfig;

export const docsSidebarTopicOptions = {
  topics: Object.fromEntries(
    topics.map((topic) => [
      topic.id,
      topic.scope.flatMap(({ nested, prefix }) =>
        languages.flatMap((language) => {
          const id = `/${docsSlug(prefix, language)}`;
          return nested ? [id, `${id}/**/*`] : [id];
        }),
      ),
    ]),
  ),
} satisfies StarlightSidebarTopicsUserOptions;

export function docsSidebarGroupIcon(
  label: string,
  language: DocsLanguage,
): string | undefined {
  for (const topic of topics) {
    const node = topic.items.find(
      (item) => !("route" in item) && item.label[language] === label,
    );
    if (node && !("route" in node)) return node.icon;
  }
  return undefined;
}

export function localizeDocsSidebar(
  sidebar: StarlightRouteData["sidebar"],
  language: DocsLanguage,
): StarlightRouteData["sidebar"] {
  const localeGroup = sidebar.find(
    (entry) =>
      entry.type === "group" && entry.label === localeGroupLabels[language],
  );
  if (localeGroup?.type !== "group") {
    throw new Error(`Missing ${language} sidebar for the current docs topic.`);
  }
  return localeGroup.entries;
}

export function localizeDocsTopics(
  routeTopics: App.Locals["starlightSidebarTopics"]["topics"],
  language: DocsLanguage,
): App.Locals["starlightSidebarTopics"]["topics"] {
  return routeTopics.map((routeTopic) => {
    const topic = topics.find(
      (candidate) => docsHref(candidate.link, "en") === routeTopic.link,
    );
    if (!topic) {
      throw new Error(`Unknown docs topic link: ${routeTopic.link}`);
    }
    return {
      ...routeTopic,
      label: topic.label[language],
      link: docsHref(topic.link, language),
    };
  });
}

export function flattenDocsSidebar(
  sidebar: StarlightRouteData["sidebar"],
): Extract<StarlightRouteData["sidebar"][number], { type: "link" }>[] {
  return sidebar.flatMap((entry) =>
    entry.type === "link" ? [entry] : flattenDocsSidebar(entry.entries),
  );
}
