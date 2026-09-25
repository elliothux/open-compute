import { useEffect, useRef, useState, type CSSProperties } from "react";
import type { CapabilityId, HomeMessages } from "../../i18n/home";
import { ScrambleLabel, SectionMeta, useScrambleText } from "../primitives";
import styles from "./styles.module.css";

type Feature = {
  id: CapabilityId;
  label: string;
  eyebrow: string;
  title: string;
  bullets: readonly string[];
  image: string;
  visual:
    | {
        type: "code";
        examples: readonly CodeExample[];
      }
    | {
        type: "screenshot";
        src: string;
      };
};

type CodeExample = {
  code: string;
  file: string;
  language: string;
};

type CodeVisual = Extract<Feature["visual"], { type: "code" }>;

type FeaturePresentation = Pick<Feature, "id" | "image" | "visual">;

const featurePresentation: readonly FeaturePresentation[] = [
  {
    id: "deploy",
    visual: {
      type: "code",
      examples: [
        {
          file: "DEPLOY / PRODUCTION",
          language: "READY",
          code: `$ ocd status
$ ocd wrangler deploy --env production

✓ uploaded 12 modules
✓ deployment v42 active
✓ /api/* → v42

$ ocd caddy status
READY    shared HTTPS Gateway`,
        },
      ],
    },
    image: "/assets/capabilities/deploy.webp",
  },
  {
    id: "runtime",
    visual: {
      type: "code",
      examples: [
        {
          file: "SRC / INDEX.TS",
          language: "TYPESCRIPT",
          code: `type Env = {
  CACHE: KVNamespace;
  DB: D1Database;
  BUCKET: R2Bucket;
};

export default {
  async fetch(request: Request, env: Env) {
    const profile = await env.CACHE.get("user:42", "json");
    const row = await env.DB.prepare("SELECT * FROM jobs").first();
    await env.BUCKET.put("request.json", request.body);
    return Response.json({ profile, row });
  },
} satisfies ExportedHandler<Env>;`,
        },
        {
          file: "SRC / MAIN.PY",
          language: "PYTHON",
          code: `from workers import WorkerEntrypoint, Response

class Default(WorkerEntrypoint):
    async def fetch(self, request):
        tables = await self.env.DB.prepare("PRAGMA table_list").run()
        await self.env.QUEUE.send({"source": "python"})
        return Response.json(tables)`,
        },
        {
          file: "SRC / LIB.RS",
          language: "RUST",
          code: `use worker::*;

#[event(fetch)]
async fn main(
    _request: Request,
    env: Env,
    _ctx: Context,
) -> Result<Response> {
    let greeting = env.kv("CACHE")?
        .get("greeting")
        .text()
        .await?;
    Response::ok(greeting.unwrap_or("Hello from Rust!".into()))
}`,
        },
      ],
    },
    image: "/assets/capabilities/worker-apis.webp",
  },
  {
    id: "extensions",
    visual: {
      type: "code",
      examples: [
        {
          file: "NATIVE EXTENSION / DATA PATH",
          language: "DIRECT",
          code: `[extensions.local-files]
path = "./extensions/files"

Worker
  → Service Binding + props
  → JavaScript facade
  → Cap'n Proto session
  → native Provider

ocd authenticates the session,
then leaves the business data path.`,
        },
      ],
    },
    image: "/assets/capabilities/extensions.webp",
  },
  {
    id: "dashboard",
    visual: {
      type: "screenshot",
      src: "/assets/capabilities/dashboard.webp",
    },
    image: "/assets/capabilities/bindings.webp",
  },
  {
    id: "operate",
    visual: {
      type: "code",
      examples: [
        {
          file: "LOCAL AUTHORITY",
          language: "HEALTHY",
          code: `$ ocd status
READY    workerd supervised
STORAGE  local authority

$ ocd --instance production backup list --json
✓ snapshots verified

$ ocd upgrade --dry-run
✓ active instances ready to upgrade`,
        },
      ],
    },
    image: "/assets/capabilities/operate.webp",
  },
];

const tokenPattern =
  /(\/\/.*$)|("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`)|(\b(?:type|export|default|const|return|await|satisfies|true|false|from|import|class|async|def|use|fn|let|mut)\b)|(\b(?:Env|Request|Response|URL|KVNamespace|D1Database|R2Bucket|Queue|ExportedHandler|WorkerEntrypoint|Result|Context|Default)\b)|(\b\d+(?:\.\d+)?\b)|(^\$)|(^[✓→])/gm;

function HighlightedCode({ code }: { code: string }) {
  const nodes: React.ReactNode[] = [];
  let cursor = 0;

  for (const match of code.matchAll(tokenPattern)) {
    const index = match.index ?? 0;
    if (index > cursor) nodes.push(code.slice(cursor, index));

    const className = match[1]
      ? "syntax-comment"
      : match[2]
        ? "syntax-string"
        : match[3]
          ? "syntax-keyword"
          : match[4]
            ? "syntax-type"
            : match[5]
              ? "syntax-number"
              : match[6]
                ? "syntax-prompt"
                : "syntax-success";

    nodes.push(
      <span className={className} key={`${index}-${match[0]}`}>
        {match[0]}
      </span>,
    );
    cursor = index + match[0].length;
  }

  if (cursor < code.length) nodes.push(code.slice(cursor));
  return nodes;
}

function CodeCard({
  ariaLabel,
  visual,
}: {
  ariaLabel: string;
  visual: CodeVisual;
}) {
  const [language, setLanguage] = useState(visual.examples[0]?.language);
  const example =
    visual.examples.find((candidate) => candidate.language === language) ??
    visual.examples[0];

  if (!example) return null;

  return (
    <div className="capabilities__code-card">
      <div className="capabilities__code-bar mono">
        <span>{example.file}</span>
        {visual.examples.length === 1 ? (
          <span className="capabilities__code-status">{example.language}</span>
        ) : (
          <span
            aria-label={ariaLabel}
            className="capabilities__code-languages"
            role="group"
          >
            {visual.examples.map((candidate) => (
              <button
                aria-pressed={candidate.language === example.language}
                className={
                  candidate.language === example.language
                    ? "is-active"
                    : undefined
                }
                key={candidate.language}
                onClick={() => setLanguage(candidate.language)}
                type="button"
              >
                {candidate.language}
              </button>
            ))}
          </span>
        )}
      </div>
      <pre>
        <code>
          <HighlightedCode code={example.code} />
        </code>
      </pre>
    </div>
  );
}

// Pixelarticons by Gerrit Halfmann, MIT licensed.
// Source: https://github.com/halfmage/pixelarticons
function FeatureIcon({ id }: { id: Feature["id"] }) {
  if (id === "deploy") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" fill="currentColor">
        <path d="M19 21H5v-2h14v2ZM5 19H3v-4h2v4Zm16 0h-2v-4h2v4ZM13 5h2v2h2v2h-4v8h-2V9H7V7h2V5h2V3h2v2Z" />
      </svg>
    );
  }

  if (id === "operate") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" fill="currentColor">
        <path d="M4 2h16v2H4zm0 18h16v2H4zM2 4h2v16H2zm18 0h2v16h-2zM6 16h2v2H6zm2-2h2v2H8zm-2-2h2v2H6z" />
      </svg>
    );
  }

  if (id === "runtime") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" fill="currentColor">
        <path d="M11 18H9v-4h2v4Zm-4-1H5v-2h2v2Zm12-2v2h-2v-2h2ZM5 15H3v-2h2v2Zm16 0h-2v-2h2v2Zm-8-1h-2v-4h2v4ZM3 13H1v-2h2v2Zm20 0h-2v-2h2v2ZM5 11H3V9h2v2Zm16 0h-2V9h2v2Zm-6-1h-2V6h2v4ZM7 9H5V7h2v2Zm12 0h-2V7h2v2Z" />
      </svg>
    );
  }

  if (id === "dashboard") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" fill="currentColor">
        <path d="M1.5 6.50098V21.5044H2.50023V22.5046H3.50045V23.5048H21.5045V22.5046H22.5048V21.5044H23.505V6.50098H1.5ZM9.50182 9.50166H8.50159V10.5019H7.50136V12.5023H6.50114V13.5026H5.50091V15.503H6.50114V16.5033H7.50136V18.5037H8.50159V19.5039H9.50182V20.5042H7.50136V19.5039H6.50114V18.5037H5.50091V16.5033H4.50068V15.503H3.50045V13.5026H4.50068V12.5023H5.50091V10.5019H6.50114V9.50166H7.50136V8.50143H9.50182V9.50166ZM14.503 12.5023H13.5027V16.5033H12.5025V20.5042H11.5023V21.5044H10.502V16.5033H11.5023V13.5026H12.5025V8.50143H13.5027V7.5012H14.503V12.5023ZM21.5045 15.503H20.5043V16.5033H19.5041V18.5037H18.5039V19.5039H17.5036V20.5042H15.5032V19.5039H16.5034V18.5037H17.5036V16.5033H18.5039V15.503H19.5041V13.5026H18.5039V12.5023H17.5036V10.5019H16.5034V9.50166H15.5032V8.50143H17.5036V9.50166H18.5039V10.5019H19.5041V12.5023H20.5043V13.5026H21.5045V15.503Z" />
        <path d="M22.5048 3.50045V2.50023H21.5045V1.5H3.50045V2.50023H2.50023V3.50045H1.5V5.50091H23.505V3.50045H22.5048ZM4.50068 4.50068V2.50023H6.50114V4.50068H4.50068ZM7.50136 4.50068V2.50023H9.50182V4.50068H7.50136ZM10.502 4.50068V2.50023H12.5025V4.50068H10.502Z" />
      </svg>
    );
  }

  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" fill="currentColor">
      <path d="M4 6h7v2H4zm0 10h7v2H4zM2 8h2v8H2zm18-2h-7v2h7zm0 10h-7v2h7zm2-8h-2v8h2zM7 11h10v2H7z" />
    </svg>
  );
}

function PixelCheckIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" fill="currentColor">
      <path d="M10 18H8v-2h2v2Zm-2-2H6v-2h2v2Zm4-2v2h-2v-2h2Zm-6 0H4v-2h2v2Zm8 0h-2v-2h2v2Zm2-2h-2v-2h2v2Zm2-2h-2V8h2v2Zm2-2h-2V6h2v2Z" />
    </svg>
  );
}

function FeatureTab({
  feature,
  active,
  onSelect,
}: {
  feature: Feature;
  active: boolean;
  onSelect: () => void;
}) {
  const [label, scramble] = useScrambleText(feature.label);
  const wasActiveRef = useRef(false);

  useEffect(() => {
    if (active && !wasActiveRef.current) scramble();
    wasActiveRef.current = active;
  }, [active, scramble]);

  return (
    <a
      className={active ? "is-active" : undefined}
      href={`#capability-${feature.id}`}
      aria-current={active ? "true" : undefined}
      onClick={(event) => {
        event.preventDefault();
        onSelect();
      }}
      onFocus={scramble}
      onMouseEnter={scramble}
    >
      <FeatureIcon id={feature.id} />
      <ScrambleLabel value={feature.label}>{label}</ScrambleLabel>
    </a>
  );
}

export function CapabilitiesSection({
  messages,
}: {
  messages: HomeMessages["capabilities"];
}) {
  const features: readonly Feature[] = featurePresentation.map((feature) => ({
    ...feature,
    ...messages.features[feature.id],
  }));
  const [active, setActive] = useState(0);
  const chapterRefs = useRef<(HTMLElement | null)[]>([]);
  const sectionRef = useRef<HTMLElement>(null);
  const tabsRef = useRef<HTMLElement>(null);

  useEffect(() => {
    const chapters = chapterRefs.current.filter(
      (chapter): chapter is HTMLElement => chapter !== null,
    );
    const observer = new IntersectionObserver(
      (entries) => {
        const current = entries
          .filter((entry) => entry.isIntersecting)
          .sort((a, b) => b.intersectionRatio - a.intersectionRatio)[0];

        if (current) {
          setActive(Number((current.target as HTMLElement).dataset.index));
        }
      },
      { rootMargin: "-18% 0px -58% 0px", threshold: [0, 0.15, 0.35] },
    );

    chapters.forEach((chapter) => observer.observe(chapter));
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    let frame = 0;

    const updatePinnedState = () => {
      window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(() => {
        const section = sectionRef.current;
        const tabs = tabsRef.current;
        if (!section || !tabs) return;

        const tabsRect = tabs.getBoundingClientRect();
        const sectionRect = section.getBoundingClientRect();
        const isPinned =
          tabsRect.top <= 0.5 && sectionRect.bottom > tabsRect.height;
        document.body.classList.toggle("capabilities-tabs-pinned", isPinned);
      });
    };

    updatePinnedState();
    window.addEventListener("scroll", updatePinnedState, { passive: true });
    window.addEventListener("resize", updatePinnedState);

    return () => {
      window.cancelAnimationFrame(frame);
      window.removeEventListener("scroll", updatePinnedState);
      window.removeEventListener("resize", updatePinnedState);
      document.body.classList.remove("capabilities-tabs-pinned");
    };
  }, []);

  useEffect(() => {
    const tabs = tabsRef.current;
    const activeTab = tabs?.children.item(active);
    if (
      !tabs ||
      !(activeTab instanceof HTMLElement) ||
      tabs.scrollWidth <= tabs.clientWidth
    ) {
      return;
    }

    const target =
      activeTab.offsetLeft - (tabs.clientWidth - activeTab.offsetWidth) / 2;
    const maximum = tabs.scrollWidth - tabs.clientWidth;
    tabs.scrollTo({
      behavior: "smooth",
      left: Math.max(0, Math.min(target, maximum)),
    });
  }, [active]);

  return (
    <section
      className={`capabilities ${styles.module}`}
      id="capabilities"
      ref={sectionRef}
    >
      <div className="section-shell capabilities__inner">
        <SectionMeta index="02" label={messages.label} dark />
        <div className="capabilities__heading">
          <h2>{messages.heading}</h2>
        </div>

        <nav
          className="capabilities__sticky-tabs"
          aria-label={messages.ariaLabel}
          ref={tabsRef}
        >
          {features.map((feature, index) => (
            <FeatureTab
              active={index === active}
              feature={feature}
              key={feature.id}
              onSelect={() => {
                setActive(index);
                chapterRefs.current[index]?.scrollIntoView({
                  behavior: "smooth",
                  block: "start",
                });
              }}
            />
          ))}
        </nav>

        <div className="capabilities__chapters">
          {features.map((feature, index) => (
            <article
              className="capabilities__chapter"
              id={`capability-${feature.id}`}
              data-index={index}
              key={feature.id}
              ref={(node) => {
                chapterRefs.current[index] = node;
              }}
            >
              <div className="capabilities__copy">
                <div>
                  <div className="capabilities__eyebrow-row">
                    <span className="capabilities__eyebrow mono">
                      {feature.eyebrow}
                    </span>
                    {feature.id === "dashboard" ? (
                      <span className="capabilities__wip-tag mono">WIP</span>
                    ) : null}
                  </div>
                  <h3>{feature.title}</h3>
                </div>
                <ul>
                  {feature.bullets.map((bullet) => (
                    <li key={bullet}>
                      <PixelCheckIcon />
                      {bullet}
                    </li>
                  ))}
                </ul>
              </div>

              <div
                className="capabilities__visual"
                style={
                  {
                    "--feature-image": `url(${feature.image})`,
                  } as CSSProperties
                }
              >
                {feature.visual.type === "screenshot" ? (
                  <div className="capabilities__screenshot-card">
                    <img
                      alt=""
                      decoding="async"
                      loading="lazy"
                      src={feature.visual.src}
                    />
                  </div>
                ) : (
                  <CodeCard
                    ariaLabel={messages.codeLanguageAriaLabel}
                    visual={feature.visual}
                  />
                )}
              </div>
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}
