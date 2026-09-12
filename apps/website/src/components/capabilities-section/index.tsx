import { useEffect, useRef, useState, type CSSProperties } from "react";
import { ScrambleLabel, SectionMeta, useScrambleText } from "../primitives";
import styles from "./styles.module.css";

type Feature = {
  id: string;
  label: string;
  eyebrow: string;
  title: string;
  bullets: string[];
  codeLabel: string;
  codeStatus: string;
  code: string;
  image: string;
};

const features: Feature[] = [
  {
    id: "deploy",
    label: "Deploy",
    eyebrow: "OCD + WRANGLER",
    title: "Keep your project. Change the target.",
    bullets: [
      "Same wrangler.jsonc",
      "Immutable deployments and rollback",
      "Local or remote infrastructure",
    ],
    codeLabel: "DEPLOY / PRODUCTION",
    codeStatus: "READY",
    code: `$ ocd target add local --instance default
$ ocd target use local
$ ocd wrangler deploy --env production

✓ uploaded 12 modules
✓ deployment v42 active
✓ /api/* → v42`,
    image: "/assets/capabilities/deploy.webp",
  },
  {
    id: "worker-apis",
    label: "Runtime",
    eyebrow: "WORKER RUNTIME",
    title: "Standard Worker runtime.",
    bullets: [
      "Fetch, Streams, Crypto, and WebSockets",
      "Standard module Worker syntax",
      "Isolates instead of containers",
    ],
    codeLabel: "SRC / INDEX.TS",
    codeStatus: "TYPESCRIPT",
    code: `type Env = {
  GREETING: string;
};

export default {
  fetch(request: Request, env: Env) {
    return Response.json({
      message: env.GREETING,
      path: new URL(request.url).pathname,
    });
  },
} satisfies ExportedHandler<Env>;`,
    image: "/assets/capabilities/worker-apis.webp",
  },
  {
    id: "bindings",
    label: "Bindings",
    eyebrow: "STATE + SERVICES",
    title: "Bindings through env.",
    bullets: [
      "Familiar Cloudflare binding APIs",
      "Capabilities declared in Wrangler",
      "Local or S3-backed authority",
    ],
    codeLabel: "BINDINGS / ENV",
    codeStatus: "CONNECTED",
    code: `type Env = {
  CACHE: KVNamespace;
  DB: D1Database;
  BUCKET: R2Bucket;
  JOBS: Queue<Job>;
};

const profile = await env.CACHE.get("user:42", "json");

const row = await env.DB
  .prepare("SELECT * FROM jobs WHERE id = ?")
  .bind(id)
  .first();

await env.BUCKET.put(key, request.body);
await env.JOBS.send({ id });`,
    image: "/assets/capabilities/bindings.webp",
  },
  {
    id: "operate",
    label: "Operate",
    eyebrow: "OCD",
    title: "One local control plane.",
    bullets: [
      "One binary and one managed service",
      "Supervised workerd runtime",
      "One-time Dashboard login",
    ],
    codeLabel: "LOCAL AUTHORITY",
    codeStatus: "HEALTHY",
    code: `$ ocd setup --yes
✓ service configured
✓ runtime ready

$ ocd status
READY    workerd supervised
STORAGE  local authority

$ ocd dashboard
→ opening one-time login`,
    image: "/assets/capabilities/operate.webp",
  },
];

const tokenPattern =
  /(\/\/.*$)|("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`)|(\b(?:type|export|default|const|return|await|satisfies|true|false)\b)|(\b(?:Env|Request|Response|URL|KVNamespace|D1Database|R2Bucket|Queue|ExportedHandler)\b)|(\b\d+(?:\.\d+)?\b)|(^\$)|(^[✓→])/gm;

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

  if (id === "worker-apis") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" fill="currentColor">
        <path d="M11 18H9v-4h2v4Zm-4-1H5v-2h2v2Zm12-2v2h-2v-2h2ZM5 15H3v-2h2v2Zm16 0h-2v-2h2v2Zm-8-1h-2v-4h2v4ZM3 13H1v-2h2v2Zm20 0h-2v-2h2v2ZM5 11H3V9h2v2Zm16 0h-2V9h2v2Zm-6-1h-2V6h2v4ZM7 9H5V7h2v2Zm12 0h-2V7h2v2Z" />
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

export function CapabilitiesSection() {
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
        <SectionMeta index="02" label="PLATFORM" dark />
        <div className="capabilities__heading">
          <h2>Deploy, run, and operate Workers.</h2>
        </div>

        <nav
          className="capabilities__sticky-tabs"
          aria-label="Platform features"
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
                  <span className="capabilities__eyebrow mono">
                    {feature.eyebrow}
                  </span>
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
                <div className="capabilities__code-card">
                  <div className="capabilities__code-bar mono">
                    <span>{feature.codeLabel}</span>
                    <span>{feature.codeStatus}</span>
                  </div>
                  <pre>
                    <code>
                      <HighlightedCode code={feature.code} />
                    </code>
                  </pre>
                </div>
              </div>
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}
