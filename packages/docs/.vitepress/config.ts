import { defineConfig } from "vitepress";

const zhSidebar = [
  {
    text: "了解",
    items: [
      { text: "概述", link: "/" },
      { text: "能力与限制", link: "/capabilities" },
    ],
  },
  {
    text: "上手",
    items: [
      { text: "安装与首次启动", link: "/install" },
      { text: "配置", link: "/configuration" },
      { text: "部署", link: "/deploy" },
    ],
  },
  {
    text: "日常",
    items: [
      { text: "健康检查", link: "/health" },
      { text: "备份与保留", link: "/backup" },
      { text: "常用命令", link: "/cli" },
    ],
  },
  {
    text: "故障手册",
    items: [
      { text: "怎么用", link: "/incidents/" },
      { text: "当前 release 恢复", link: "/incidents/current-release" },
      { text: "全新主机恢复", link: "/incidents/fresh-host" },
      { text: "磁盘压力", link: "/incidents/disk" },
      { text: "SQLite 损坏", link: "/incidents/sqlite" },
      { text: "S3 故障", link: "/incidents/s3" },
      { text: "workerd 崩溃循环", link: "/incidents/workerd" },
      { text: "Master key 丢失", link: "/incidents/master-key" },
      { text: "Scheduler 恢复", link: "/incidents/scheduler" },
      { text: "收集 support bundle", link: "/incidents/support-bundle" },
    ],
  },
];

const enSidebar = [
  {
    text: "Understand",
    items: [
      { text: "Overview", link: "/en/" },
      { text: "Capabilities and limits", link: "/en/capabilities" },
    ],
  },
  {
    text: "Get started",
    items: [
      { text: "Install and first start", link: "/en/install" },
      { text: "Configuration", link: "/en/configuration" },
      { text: "Deploy", link: "/en/deploy" },
    ],
  },
  {
    text: "Operations",
    items: [
      { text: "Health checks", link: "/en/health" },
      { text: "Backup and retention", link: "/en/backup" },
      { text: "CLI reference", link: "/en/cli" },
    ],
  },
  {
    text: "Incident handbook",
    items: [
      { text: "How to use", link: "/en/incidents/" },
      { text: "Current-release restore", link: "/en/incidents/current-release" },
      { text: "Fresh-host restore", link: "/en/incidents/fresh-host" },
      { text: "Disk pressure", link: "/en/incidents/disk" },
      { text: "SQLite corruption", link: "/en/incidents/sqlite" },
      { text: "S3 outage", link: "/en/incidents/s3" },
      { text: "workerd crash loop", link: "/en/incidents/workerd" },
      { text: "Master-key loss", link: "/en/incidents/master-key" },
      { text: "Scheduler recovery", link: "/en/incidents/scheduler" },
      { text: "Collect a support bundle", link: "/en/incidents/support-bundle" },
    ],
  },
];

export default defineConfig({
  title: "open-compute",
  lastUpdated: false,
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
      description: "platformd 运维文档",
      themeConfig: {
        nav: [{ text: "概述", link: "/" }, { text: "故障手册", link: "/incidents/" }],
        sidebar: zhSidebar,
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
      description: "platformd operator documentation",
      themeConfig: {
        nav: [{ text: "Overview", link: "/en/" }, { text: "Incident handbook", link: "/en/incidents/" }],
        sidebar: enSidebar,
        outline: { label: "On this page", level: [2, 3] },
        langMenuLabel: "Change language",
      },
    },
  },
});
