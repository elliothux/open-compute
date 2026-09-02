import { defineConfig, type DefaultTheme } from "vitepress";

type Copy = {
  start: string;
  overview: string;
  getStarted: string;
  directory: string;
  concepts: string;
  guides: string;
  examples: string;
  configuration: string;
  bindings: string;
  compatibilityDates: string;
  compatibilityFlags: string;
  cronTriggers: string;
  envVars: string;
  secrets: string;
  routing: string;
  versions: string;
  staticAssets: string;
  cache: string;
  runtimeApis: string;
  handlers: string;
  websockets: string;
  tcpSockets: string;
  nodejs: string;
  platform: string;
  limits: string;
  knownIssues: string;
  changelog: string;
  storage: string;
  compute: string;
  media: string;
  compatibility: string;
  deviations: string;
  unsupported: string;
  apiRef: string;
  operate: string;
  install: string;
  config: string;
  deploy: string;
  health: string;
  backup: string;
  cli: string;
  incidents: string;
  alarms: string;
};

const zhCopy: Copy = {
  start: "开始",
  overview: "概述",
  getStarted: "上手",
  directory: "产品目录",
  concepts: "概念",
  guides: "指南",
  examples: "示例",
  configuration: "配置",
  bindings: "绑定",
  compatibilityDates: "兼容日期",
  compatibilityFlags: "兼容标志",
  cronTriggers: "Cron 触发器",
  envVars: "环境变量",
  secrets: "密钥",
  routing: "路由",
  versions: "版本与部署",
  staticAssets: "静态资源",
  cache: "缓存",
  runtimeApis: "运行时 API",
  handlers: "Handlers",
  websockets: "WebSockets",
  tcpSockets: "TCP sockets",
  nodejs: "Node.js 兼容",
  platform: "平台",
  limits: "限制",
  knownIssues: "已知问题",
  changelog: "更新日志",
  storage: "存储",
  compute: "计算",
  media: "媒体",
  compatibility: "兼容性",
  deviations: "行为差异",
  unsupported: "不支持",
  apiRef: "API 参考",
  operate: "运维 ocd",
  install: "安装与首次启动",
  config: "配置",
  deploy: "部署",
  health: "健康检查",
  backup: "备份与保留",
  cli: "常用命令",
  incidents: "故障手册",
  alarms: "Alarms",
};

const enCopy: Copy = {
  start: "Start",
  overview: "Overview",
  getStarted: "Get started",
  directory: "Directory",
  concepts: "Concepts",
  guides: "Guides",
  examples: "Examples",
  configuration: "Configuration",
  bindings: "Bindings",
  compatibilityDates: "Compatibility dates",
  compatibilityFlags: "Compatibility flags",
  cronTriggers: "Cron Triggers",
  envVars: "Environment variables",
  secrets: "Secrets",
  routing: "Routing",
  versions: "Versions and deployments",
  staticAssets: "Static Assets",
  cache: "Cache",
  runtimeApis: "Runtime APIs",
  handlers: "Handlers",
  websockets: "WebSockets",
  tcpSockets: "TCP sockets",
  nodejs: "Node.js compatibility",
  platform: "Platform",
  limits: "Limits",
  knownIssues: "Known issues",
  changelog: "Changelog",
  storage: "Storage",
  compute: "Compute",
  media: "Media",
  compatibility: "Compatibility",
  deviations: "Behavior differences",
  unsupported: "Unsupported",
  apiRef: "API reference",
  operate: "Operate ocd",
  install: "Install and first start",
  config: "Configuration",
  deploy: "Deploy",
  health: "Health checks",
  backup: "Backup and retention",
  cli: "CLI reference",
  incidents: "Incident handbook",
  alarms: "Alarms",
};

function productTree(
  prefix: string,
  t: Copy,
  slug: string,
  name: string,
  extra?: DefaultTheme.SidebarItem[],
): DefaultTheme.SidebarItem {
  return {
    text: name,
    collapsed: true,
    items: [
      { text: t.overview, link: `${prefix}/${slug}/` },
      { text: t.getStarted, link: `${prefix}/${slug}/get-started/` },
      { text: t.concepts, link: `${prefix}/${slug}/concepts/` },
      { text: t.guides, link: `${prefix}/${slug}/guides/` },
      { text: t.examples, link: `${prefix}/${slug}/examples/` },
      ...(extra ?? []),
      { text: t.limits, link: `${prefix}/${slug}/platform/limits` },
      { text: t.deviations, link: `${prefix}/${slug}/platform/deviations` },
    ],
  };
}

function sidebar(prefix: string, t: Copy): DefaultTheme.SidebarItem[] {
  return [
    {
      text: t.start,
      items: [
        { text: t.overview, link: `${prefix}/` },
        { text: t.getStarted, link: `${prefix}/get-started` },
        { text: t.directory, link: `${prefix}/directory` },
      ],
    },
    {
      text: "Workers",
      items: [
        { text: t.overview, link: `${prefix}/workers/` },
        { text: t.getStarted, link: `${prefix}/workers/get-started/` },
        { text: t.concepts, link: `${prefix}/workers/concepts/` },
        { text: t.examples, link: `${prefix}/workers/examples/` },
        {
          text: t.configuration,
          collapsed: true,
          items: [
            { text: t.overview, link: `${prefix}/workers/configuration/` },
            { text: t.bindings, link: `${prefix}/workers/configuration/bindings` },
            { text: t.compatibilityDates, link: `${prefix}/workers/configuration/compatibility-dates` },
            { text: t.compatibilityFlags, link: `${prefix}/workers/configuration/compatibility-flags` },
            { text: t.cronTriggers, link: `${prefix}/workers/configuration/cron-triggers` },
            { text: t.envVars, link: `${prefix}/workers/configuration/environment-variables` },
            { text: t.secrets, link: `${prefix}/workers/configuration/secrets` },
            { text: t.routing, link: `${prefix}/workers/configuration/routing` },
          ],
        },
        { text: t.versions, link: `${prefix}/workers/versions-and-deployments/` },
        { text: t.staticAssets, link: `${prefix}/workers/static-assets/` },
        { text: t.cache, link: `${prefix}/workers/cache/` },
        {
          text: t.runtimeApis,
          collapsed: true,
          items: [
            { text: t.overview, link: `${prefix}/workers/runtime-apis/` },
            { text: t.handlers, link: `${prefix}/workers/runtime-apis/handlers` },
            { text: t.bindings, link: `${prefix}/workers/runtime-apis/bindings` },
            { text: t.cache, link: `${prefix}/workers/runtime-apis/cache` },
            { text: t.websockets, link: `${prefix}/workers/runtime-apis/websockets` },
            { text: t.tcpSockets, link: `${prefix}/workers/runtime-apis/tcp-sockets` },
            { text: t.nodejs, link: `${prefix}/workers/runtime-apis/nodejs` },
          ],
        },
        {
          text: t.platform,
          collapsed: true,
          items: [
            { text: t.limits, link: `${prefix}/workers/platform/limits` },
            { text: t.knownIssues, link: `${prefix}/workers/platform/known-issues` },
            { text: t.changelog, link: `${prefix}/workers/platform/changelog` },
          ],
        },
      ],
    },
    {
      text: t.storage,
      items: [
        productTree(prefix, t, "kv", "KV"),
        productTree(prefix, t, "d1", "D1"),
        productTree(prefix, t, "r2", "R2"),
      ],
    },
    {
      text: t.compute,
      items: [
        productTree(prefix, t, "durable-objects", "Durable Objects", [
          { text: t.alarms, link: `${prefix}/durable-objects/alarms` },
        ]),
        productTree(prefix, t, "queues", "Queues"),
        productTree(prefix, t, "workflows", "Workflows"),
      ],
    },
    {
      text: t.media,
      items: [productTree(prefix, t, "images", "Images")],
    },
    {
      text: t.platform,
      items: [
        { text: t.overview, link: `${prefix}/platform/` },
        { text: t.compatibility, link: `${prefix}/platform/compatibility` },
        { text: t.deviations, link: `${prefix}/platform/deviations` },
        { text: t.limits, link: `${prefix}/platform/limits` },
        { text: t.unsupported, link: `${prefix}/platform/unsupported` },
        { text: t.apiRef, link: `${prefix}/platform/reference/api/` },
      ],
    },
    {
      text: t.operate,
      items: [
        { text: t.overview, link: `${prefix}/ocd/` },
        { text: t.install, link: `${prefix}/ocd/get-started` },
        { text: t.config, link: `${prefix}/ocd/configuration` },
        { text: t.deploy, link: `${prefix}/ocd/deploy` },
        { text: t.health, link: `${prefix}/ocd/health` },
        { text: t.backup, link: `${prefix}/ocd/backup` },
        { text: t.cli, link: `${prefix}/ocd/cli` },
        { text: t.incidents, link: `${prefix}/ocd/incidents/` },
      ],
    },
  ];
}

export default defineConfig({
  title: "open-compute",
  sitemap: { hostname: "https://open-compute.dev" },
  lastUpdated: false,
  ignoreDeadLinks: false,
  srcExclude: ["README.md"],
  vite: {
    server: {
      host: "0.0.0.0",
      port: 5173,
      strictPort: true,
    },
  },
  locales: {
    root: {
      label: "简体中文",
      lang: "zh-CN",
      description: "open-compute 开发者文档",
      themeConfig: {
        nav: [
          { text: "开始", link: "/get-started" },
          { text: "产品目录", link: "/directory" },
          { text: "ocd", link: "/ocd/" },
        ],
        sidebar: sidebar("", zhCopy),
        outline: { label: "本页", level: [2, 3] },
        docFooter: { prev: "上一页", next: "下一页" },
        darkModeSwitchLabel: "外观",
        sidebarMenuLabel: "菜单",
        returnToTopLabel: "回到顶部",
        langMenuLabel: "切换语言",
      },
    },
    en: {
      label: "English",
      lang: "en-US",
      description: "open-compute developer documentation",
      themeConfig: {
        nav: [
          { text: "Get started", link: "/en/get-started" },
          { text: "Directory", link: "/en/directory" },
          { text: "ocd", link: "/en/ocd/" },
        ],
        sidebar: sidebar("/en", enCopy),
        outline: { label: "On this page", level: [2, 3] },
        langMenuLabel: "Change language",
      },
    },
  },
});
