import type {
  StarlightSidebarTopicsUserConfig,
  StarlightSidebarTopicsUserOptions,
} from "starlight-sidebar-topics";
import {
  localeConfig,
  docsHref as localizedDocsHref,
  type Locale,
} from "./i18n/config";

type Label = Record<Locale, string>;
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
  | { label: string; slug: string; translations: Record<string, string> }
  | {
      collapsed: boolean;
      items: TopicSidebarItem[];
      label: string;
      translations: Record<string, string>;
    };

const languages: Locale[] = ["en", "zh"];

const topicIcons = {
  extension: "i-tabler:puzzle",
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
  extension: "i-tabler:plug",
  gateway: "i-tabler:world-www",
  mediaAi: "i-tabler:sparkles",
  operate: "i-tabler:server-cog",
  overview: "i-tabler:layout-grid",
  platform: "i-tabler:stack-2",
  project: "i-tabler:git-branch",
  reference: "i-tabler:book-2",
  reliability: "i-tabler:shield-check",
  start: "i-tabler:flag",
  storage: "i-tabler:database",
} as const;

export const docsNavigationIconClasses = [
  "i-tabler:home",
  ...Object.values(topicIcons),
  ...Object.values(groupIcons),
];

const labels = (en: string, zh: string): Label => ({ en, zh });
const starlightTranslations = (label: Label): Record<string, string> => ({
  [localeConfig.zh.htmlLang]: label.zh,
});
const topicTranslations = (label: Label): Record<string, string> => ({
  en: label.en,
  [localeConfig.zh.htmlLang]: label.zh,
});
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
        link("Routing", "路由", "/workers/configuration/routing"),
        link("Bindings", "Bindings", "/workers/configuration/bindings"),
        link(
          "Versions and rollback",
          "版本与回滚",
          "/workers/versions-and-deployments",
        ),
        link("Static assets", "静态资源", "/workers/static-assets"),
        link("Cache", "缓存", "/workers/cache"),
        link("Logs and live tail", "日志与实时 Tail", "/workers/observability"),
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
      group("Platform", "平台", groupIcons.platform, [
        link("Operator guide", "运维指南", "/operate"),
        link(
          "Architecture and boundaries",
          "架构与职责边界",
          "/ocd/architecture",
        ),
        link("Instances", "实例", "/ocd/instances"),
        link("Dashboard", "Dashboard", "/ocd/dashboard"),
        link("Configuration", "平台配置", "/ocd/configuration"),
      ]),
      group("Gateway", "Gateway", groupIcons.gateway, [
        link("Gateway overview", "Gateway 概览", "/gateway"),
        link("DNS and TLS", "DNS 与 TLS", "/gateway/dns-tls"),
        link("Caddy configuration", "Caddy 配置", "/gateway/caddy"),
      ]),
      group("Reliability", "可靠性", groupIcons.reliability, [
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
      { nested: true, prefix: "/gateway" },
      { prefix: "/cli" },
      { nested: true, prefix: "/ocd" },
    ],
  },
  {
    icon: topicIcons.extension,
    id: "extension",
    label: labels("Extension", "扩展"),
    link: "/extension",
    items: [
      group("Extension", "扩展", groupIcons.extension, [
        link("Extensions", "扩展概览", "/extension"),
        link("Implement an extension", "实现一个扩展", "/extension/tutorial"),
        link("Extension API", "扩展 API", "/extension/api"),
        link("How calls work", "调用如何发生", "/extension/architecture"),
      ]),
    ],
    scope: [{ nested: true, prefix: "/extension" }],
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
        link(
          "API and product index",
          "API 与产品索引",
          "/platform/reference/api",
        ),
        link("Management SDK", "管理 SDK", "/platform/reference/sdk"),
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

function docsSlug(route: string): string {
  return `docs${route}`;
}

function docsHref(route: string, language: Locale): string {
  return localizedDocsHref(language, route);
}

function toSidebarItem(node: NavigationNode): TopicSidebarItem {
  if ("route" in node) {
    return {
      label: node.label.en,
      slug: docsSlug(node.route),
      translations: starlightTranslations(node.label),
    };
  }

  return {
    collapsed: node.collapsed ?? false,
    items: node.items.map(toSidebarItem),
    label: node.label.en,
    translations: starlightTranslations(node.label),
  };
}

export const docsSidebarTopics = topics.map((topic) => ({
  icon: topic.icon,
  id: topic.id,
  items: topic.items.map(toSidebarItem),
  label: topicTranslations(topic.label),
  link: docsHref(topic.link, "en"),
})) satisfies StarlightSidebarTopicsUserConfig;

export const docsSidebarTopicOptions = {
  topics: Object.fromEntries(
    topics.map((topic) => [
      topic.id,
      topic.scope.flatMap(({ nested, prefix }) =>
        languages.flatMap((language) => {
          const id = docsHref(prefix, language).replace(/\/$/, "");
          return nested ? [id, `${id}/**/*`] : [id];
        }),
      ),
    ]),
  ),
} satisfies StarlightSidebarTopicsUserOptions;

export function docsSidebarGroupIcon(
  label: string,
  language: Locale,
): string | undefined {
  for (const topic of topics) {
    const node = topic.items.find(
      (item) => !("route" in item) && item.label[language] === label,
    );
    if (node && !("route" in node)) return node.icon;
  }
  return undefined;
}
