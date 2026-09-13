import type { Locale } from "./config";

export type CapabilityId = "deploy" | "worker-apis" | "bindings" | "operate";

interface CapabilityCopy {
  bullets: readonly string[];
  eyebrow: string;
  label: string;
  title: string;
}

interface BenefitCopy {
  caption: string;
  description: string;
  title: string;
}

interface PricingPlanCopy {
  action: string;
  badge: string | null;
  description: string;
  features: readonly string[];
  intro: string;
  name: string;
  price: string;
  suffix: string;
}

export interface HomeMessages {
  benefits: {
    heading: string;
    label: string;
    cards: readonly [BenefitCopy, BenefitCopy, BenefitCopy];
  };
  capabilities: {
    ariaLabel: string;
    heading: string;
    label: string;
    features: Record<CapabilityId, CapabilityCopy>;
  };
  changelog: {
    heading: string;
    label: string;
    released: string;
    summaryTemplate: string | null;
    viewAll: string;
  };
  footer: {
    ariaLabel: string;
    install: string;
    links: {
      develop: string;
      getStarted: string;
      github: string;
      operate: string;
      products: string;
      reference: string;
    };
    sloganPrefix: string;
    sloganSuffix: string;
    sloganWords: readonly string[];
  };
  hero: {
    description: string;
    github: string;
    githubAriaLabel: string;
    install: string;
    productMatrix: {
      ariaLabel: string;
      heading: string;
      label: string;
      notes: readonly [string, string, string];
      notesAriaLabel: string;
      statusAfter: string;
      statusBefore: string;
      statusLink: string;
      supportHeading: string;
    };
    sloganPrefix: string;
    sloganSuffix: string;
    sloganWords: readonly string[];
    stars: string;
  };
  metadata: {
    description: string;
    title: string;
  };
  navigation: {
    ariaLabel: string;
    close: string;
    contact: string;
    language: string;
    links: {
      develop: string;
      github: string;
      home: string;
      operate: string;
      products: string;
    };
    mobileAriaLabel: string;
    open: string;
  };
  pricing: {
    heading: string;
    label: string;
    plans: readonly [PricingPlanCopy, PricingPlanCopy, PricingPlanCopy];
  };
}

const en = {
  metadata: {
    title: "open-compute — Cloudflare-compatible infrastructure in one binary",
    description:
      "Run a complete Cloudflare Workers-compatible platform on your own hardware with one Rust-powered binary and zero extra dependencies.",
  },
  navigation: {
    ariaLabel: "Primary navigation",
    mobileAriaLabel: "Mobile navigation",
    open: "Open navigation",
    close: "Close navigation",
    contact: "GET STARTED",
    language: "简体中文",
    links: {
      home: "OPEN-COMPUTE",
      develop: "DEVELOP",
      operate: "OPERATE",
      products: "PRODUCTS",
      github: "GITHUB",
    },
  },
  hero: {
    sloganPrefix: "The open-source cloud",
    sloganWords: [
      "for AI apps",
      "for agentic workflows",
      "for edge workloads",
      "for AI-native builders",
    ],
    sloganSuffix: "on your infrastructure.",
    description:
      "Run a complete Cloudflare Workers-compatible platform on your own hardware with one Rust-powered binary.",
    githubAriaLabel: "open-compute on GitHub",
    stars: "STARS",
    install: "INSTALL",
    github: "GITHUB",
    productMatrix: {
      ariaLabel: "Cloudflare-compatible products",
      label: "CLOUDFLARE COMPATIBILITY",
      heading: "Cloudflare platform coverage.",
      supportHeading: "Works with your ecosystem",
      statusBefore:
        "Keep your Worker code, Wrangler configuration, framework adapters, and bindings. See the",
      statusLink: "current product status",
      statusAfter: "before deploying.",
      notesAriaLabel: "Product notes",
      notes: [
        "#1 Requires an external LLM API.",
        "#2 Requires an external sidecar.",
        "#3 Partial or work in progress; see documentation.",
      ],
    },
  },
  capabilities: {
    label: "PLATFORM",
    heading: "Deploy, run, and operate Workers.",
    ariaLabel: "Platform features",
    features: {
      deploy: {
        label: "Deploy",
        eyebrow: "OCD + WRANGLER",
        title: "Keep your project. Change the target.",
        bullets: [
          "Same wrangler.jsonc",
          "Immutable deployments and rollback",
          "Local or remote infrastructure",
        ],
      },
      "worker-apis": {
        label: "Runtime",
        eyebrow: "WORKER RUNTIME",
        title: "Standard Worker runtime.",
        bullets: [
          "Fetch, Streams, Crypto, and WebSockets",
          "Standard module Worker syntax",
          "Isolates instead of containers",
        ],
      },
      bindings: {
        label: "Bindings",
        eyebrow: "STATE + SERVICES",
        title: "Bindings through env.",
        bullets: [
          "Familiar Cloudflare binding APIs",
          "Capabilities declared in Wrangler",
          "Local or S3-backed authority",
        ],
      },
      operate: {
        label: "Operate",
        eyebrow: "OCD",
        title: "One local control plane.",
        bullets: [
          "One binary and one managed service",
          "Supervised workerd runtime",
          "One-time Dashboard login",
        ],
      },
    },
  },
  benefits: {
    label: "ARCHITECTURE",
    heading: "A compact stack for self-hosted Workers.",
    cards: [
      {
        title: "Isolates, not containers.",
        description:
          "Native workerd isolation without a container per request.",
        caption: "ISOLATION",
      },
      {
        title: "One binary. No sidecars.",
        description:
          "Runtime, control plane, storage integration, and Dashboard ship together.",
        caption: "OPERATIONS",
      },
      {
        title: "Authority stays local.",
        description:
          "SQLite is authoritative. Deployments are immutable. Artifacts are content-addressed.",
        caption: "STATE",
      },
    ],
  },
  pricing: {
    label: "PRICING",
    heading: "Self-host for free.",
    plans: [
      {
        name: "Self-host",
        badge: null,
        description: "For teams running their own infrastructure.",
        price: "$0",
        suffix: "forever",
        intro: "Everything you need to self-host:",
        features: [
          "Apache-2.0 open-source platform",
          "Compatible runtime and product bindings",
          "Community support on GitHub",
        ],
        action: "GET STARTED",
      },
      {
        name: "Managed",
        badge: null,
        description:
          "For teams that want open-compute without managing infrastructure.",
        price: "Cloud",
        suffix: "",
        intro: "A managed open-compute experience:",
        features: [
          "Hosted open-compute infrastructure",
          "Managed upgrades and backups",
          "Cloud dashboard and team access",
        ],
        action: "COMING SOON",
      },
      {
        name: "Enterprise",
        badge: null,
        description: "For production teams that want direct support.",
        price: "Custom",
        suffix: "let's talk",
        intro: "Everything in Self-host, plus:",
        features: [
          "Deployment and migration guidance",
          "Security and production-readiness review",
          "Direct engineering support",
        ],
        action: "CONTACT US",
      },
    ],
  },
  changelog: {
    label: "RECENT RELEASES",
    heading: "Recent releases.",
    viewAll: "VIEW ALL",
    released: "RELEASED",
    summaryTemplate: null,
  },
  footer: {
    ariaLabel: "Footer navigation",
    sloganPrefix: "The open-source cloud",
    sloganWords: ["for AI workloads", "for APIs", "for full-stack apps"],
    sloganSuffix: "on your infrastructure.",
    install: "INSTALL",
    links: {
      getStarted: "Get started",
      develop: "Develop",
      operate: "Operate",
      products: "Products",
      reference: "Reference",
      github: "GitHub",
    },
  },
} satisfies HomeMessages;

const zh = {
  metadata: {
    title: "open-compute — 一个二进制，运行 Cloudflare 兼容基础设施",
    description:
      "一个 Rust 二进制，就能在自己的硬件上运行完整的 Cloudflare Workers 兼容平台，无需额外依赖。",
  },
  navigation: {
    ariaLabel: "主导航",
    mobileAriaLabel: "移动端导航",
    open: "打开导航",
    close: "关闭导航",
    contact: "开始使用",
    language: "ENGLISH",
    links: {
      home: "OPEN-COMPUTE",
      develop: "开发",
      operate: "运维",
      products: "产品",
      github: "GITHUB",
    },
  },
  hero: {
    sloganPrefix: "开源云基础设施",
    sloganWords: [
      "为 AI 应用而生",
      "驱动 Agent 工作流",
      "承载边缘工作负载",
      "为 AI 原生开发者打造",
    ],
    sloganSuffix: "运行在你自己的硬件。",
    description:
      "一个 Rust 二进制，就能在自己的硬件上运行完整的 Cloudflare Workers 兼容平台。",
    githubAriaLabel: "GitHub 上的 open-compute",
    stars: "星标",
    install: "安装",
    github: "GITHUB",
    productMatrix: {
      ariaLabel: "Cloudflare 兼容产品",
      label: "CLOUDFLARE 兼容",
      heading: "Cloudflare 平台能力支持一览。",
      supportHeading: "无缝兼容你熟悉的技术栈",
      statusBefore:
        "Worker 代码、Wrangler 配置、框架适配器和 Bindings 都能继续使用。部署前请先查看",
      statusLink: "当前产品状态",
      statusAfter: "。",
      notesAriaLabel: "产品说明",
      notes: [
        "#1 需要外部 LLM API。",
        "#2 需要另行部署 sidecar 服务。",
        "#3 仍处于开发中；详情请参阅文档。",
      ],
    },
  },
  capabilities: {
    label: "平台",
    heading: "一套平台，完成 Workers 的部署、运行和运维。",
    ariaLabel: "平台能力",
    features: {
      deploy: {
        label: "部署",
        eyebrow: "OCD + WRANGLER",
        title: "无需修改一行代码，只需切换部署目标。",
        bullets: [
          "沿用现有的 wrangler.jsonc",
          "每次部署都可追溯、可回滚",
          "既可本机运行，也可部署到远程主机",
        ],
      },
      "worker-apis": {
        label: "运行时",
        eyebrow: "WORKER 运行时",
        title: "熟悉的 Worker 运行时。",
        bullets: [
          "Fetch、Streams、Crypto、WebSockets 等 Web API",
          "标准 ES Module Worker 写法",
          "基于 Isolate 隔离，无需为请求启动容器",
        ],
      },
      bindings: {
        label: "Bindings",
        eyebrow: "状态与服务",
        title: "支持所有的 Bindings 类型。",
        bullets: [
          "沿用 Cloudflare Bindings API",
          "在 Wrangler 中声明所需能力",
          "数据由本地存储或 S3 托管",
        ],
      },
      operate: {
        label: "运维",
        eyebrow: "OCD",
        title: "一个工具，管理所有服务。",
        bullets: [
          "一个二进制，只需管理一个服务",
          "统一管理 workerd 运行时",
          "一次登录即可使用 Dashboard",
        ],
      },
    },
  },
  benefits: {
    label: "架构",
    heading: "自托管 Workers 需要的一切，全都在这。",
    cards: [
      {
        title: "Isolate 隔离，无需容器。",
        description: "使用 workerd 原生隔离，不必为请求启动容器。",
        caption: "隔离",
      },
      {
        title: "一个二进制，无需 sidecar。",
        description: "运行时、控制面、存储集成和 Dashboard 一起交付。",
        caption: "运维",
      },
      {
        title: "数据与控制权都留在本地。",
        description: "SQLite 是唯一事实来源，部署不可变，制品按内容寻址。",
        caption: "状态",
      },
    ],
  },
  pricing: {
    label: "定价",
    heading: "自己部署，永久免费。",
    plans: [
      {
        name: "自托管",
        badge: null,
        description: "适合自己维护基础设施的团队。",
        price: "$0",
        suffix: "永久",
        intro: "包含：",
        features: [
          "Apache-2.0 开源平台",
          "Cloudflare 兼容运行时与产品 Bindings",
          "GitHub 社区支持",
        ],
        action: "开始使用",
      },
      {
        name: "托管服务",
        badge: null,
        description: "想用 open-compute，又不想维护基础设施。",
        price: "Cloud",
        suffix: "",
        intro: "我们负责运行 open-compute：",
        features: [
          "由我们托管基础设施",
          "升级和备份由我们维护",
          "云端 Dashboard，支持团队协作",
        ],
        action: "即将推出",
      },
      {
        name: "企业版",
        badge: null,
        description: "面向需要专业支持的生产环境。",
        price: "定制",
        suffix: "联系我们",
        intro: "在自托管版基础上增加：",
        features: ["部署与迁移指导", "安全与生产就绪评估", "直接对接工程团队"],
        action: "联系我们",
      },
    ],
  },
  changelog: {
    label: "版本更新",
    heading: "最近更新。",
    viewAll: "查看全部",
    released: "已发布",
    summaryTemplate: null,
  },
  footer: {
    ariaLabel: "页脚导航",
    sloganPrefix: "开源云基础设施",
    sloganWords: ["承载 AI 工作负载", "运行你的 API", "服务全栈应用"],
    sloganSuffix: "由你部署，也由你掌控。",
    install: "安装",
    links: {
      getStarted: "开始使用",
      develop: "开发",
      operate: "运维",
      products: "产品",
      reference: "参考",
      github: "GitHub",
    },
  },
} satisfies HomeMessages;

export const homeMessages = { en, zh } satisfies Record<Locale, HomeMessages>;
