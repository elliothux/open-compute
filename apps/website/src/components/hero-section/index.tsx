import { useEffect, useState } from "react";
import { CapabilitiesSupport } from "../capabilities-section/capabilities-support";
import { GitHubIcon, PixelArrowTopRight } from "../icons";
import {
  ActionButton,
  ScrambleLabel,
  SectionMeta,
  useScrambleText,
  useTypewriter,
} from "../primitives";
import {
  CloudflareProductIcon,
  type CloudflareProductName,
} from "./product-icons";
import styles from "./styles.module.css";

const productNames = [
  "Workers",
  "Durable Objects",
  "KV",
  "D1",
  "R2",
  "Queues",
  "Workflows",
  "Cron Triggers",
  "Cache",
  "Images",
  "Vectorize",
  "AI Search",
  "Dynamic Workers",
  "Static Assets",
  "Service Bindings",
  "Artifacts",
  "Browser Run",
  "Containers",
  "Sandbox",
  "LogTail",
] as const satisfies readonly CloudflareProductName[];
const productFootnotes: Partial<
  Record<CloudflareProductName, readonly (1 | 2 | 3)[]>
> = {
  Vectorize: [1],
  "AI Search": [1],
  "Browser Run": [3],
  Containers: [2, 3],
  Sandbox: [2, 3],
};
const heroWords = [
  "for AI workloads",
  "for APIs",
  "for full-stack apps",
] as const;

interface GitHubStarsResponse {
  stars: number;
}

function ProductMatrixItem({ name }: { name: CloudflareProductName }) {
  const [label, scramble] = useScrambleText(name);
  const footnotes = productFootnotes[name];

  return (
    <div className="product-matrix__item" onMouseEnter={scramble}>
      <CloudflareProductIcon product={name} />
      <ScrambleLabel value={name}>{label}</ScrambleLabel>
      {footnotes ? (
        <span className="product-matrix__note-tags">
          {footnotes.map((footnote) => (
            <sup key={footnote}>#{footnote}</sup>
          ))}
        </span>
      ) : null}
    </div>
  );
}

function isGitHubStarsResponse(value: unknown): value is GitHubStarsResponse {
  if (typeof value !== "object" || value === null || !("stars" in value)) {
    return false;
  }

  const stars = value.stars;
  return Number.isInteger(stars) && typeof stars === "number" && stars >= 0;
}

export function HeroSection() {
  const typedWord = useTypewriter(heroWords, 100, 1000, 60);
  const [githubLabel, scrambleGithubLabel] = useScrambleText("GITHUB");
  const [stars, setStars] = useState<number | null>(null);

  useEffect(() => {
    const controller = new AbortController();

    async function loadStars() {
      try {
        const response = await fetch("/api/github-stars/", {
          signal: controller.signal,
        });
        if (!response.ok) return;

        const payload: unknown = await response.json();
        if (isGitHubStarsResponse(payload)) setStars(payload.stars);
      } catch (error: unknown) {
        if (!(error instanceof DOMException && error.name === "AbortError")) {
          setStars(null);
        }
      }
    }

    void loadStars();
    return () => controller.abort();
  }, []);

  const starsLabel =
    stars === null ? "GITHUB STARS" : `${stars.toLocaleString("en-US")} STARS`;

  return (
    <>
      <section className={`hero ${styles.module}`} id="top">
        <div className="hero__background" aria-hidden="true">
          <video
            className="hero__video"
            autoPlay
            loop
            muted
            playsInline
            poster="/videos/open-compute-hero-poster.webp"
            preload="auto"
          >
            <source src="/videos/open-compute-hero.mp4" type="video/mp4" />
          </video>
        </div>
        <div className="hero__veil" aria-hidden="true" />

        <div className="hero__inner section-shell">
          <div className="hero__top">
            <div className="hero__heading">
              <h1>The open-source cloud</h1>
              <div className="hero__typed" aria-live="polite">
                [{typedWord}
                <span className="type-caret" />]
              </div>
              <h1>on your infrastructure.</h1>
            </div>
            <div className="hero__description">
              <div className="hero__proof">
                <a
                  className="hero__github"
                  href="https://github.com/elliothux/open-compute"
                  aria-label="Open Compute on GitHub"
                >
                  <GitHubIcon />
                </a>
                <span aria-live="polite">{starsLabel}</span>
                <span className="hero__rating">APACHE-2.0</span>
              </div>
              <p>
                Run supported Cloudflare Workers projects on your infrastructure
                with one Rust-powered binary.
              </p>
              <div className="hero__actions">
                <ActionButton href="/docs/get-started/">INSTALL</ActionButton>
                <a
                  className="hero__demo mono"
                  href="https://github.com/elliothux/open-compute"
                  onFocus={scrambleGithubLabel}
                  onMouseEnter={scrambleGithubLabel}
                >
                  <span className="hero__demo-icon">
                    <PixelArrowTopRight />
                  </span>
                  <span className="hero__demo-label-wrap">
                    <span className="hero__demo-label">
                      <ScrambleLabel value="GITHUB">
                        {githubLabel}
                      </ScrambleLabel>
                    </span>
                  </span>
                </a>
              </div>
            </div>
          </div>
        </div>
      </section>

      <section
        className="product-matrix"
        id="compatibility"
        aria-label="Cloudflare-compatible products"
      >
        <div className="section-shell">
          <SectionMeta index="01" label="CLOUDFLARE COMPATIBILITY" />
          <div className="product-matrix__heading">
            <h2 className="product-matrix__title">
              Supported Cloudflare APIs.
            </h2>
            <p>
              Keep your Worker code, Wrangler configuration, framework adapters,
              and bindings.
            </p>
          </div>
          <div className="product-matrix__grid">
            {productNames.map((name) => (
              <ProductMatrixItem key={name} name={name} />
            ))}
          </div>
          <CapabilitiesSupport />
          <div className="product-matrix__notes" aria-label="Product notes">
            <span>#1 Requires an external LLM API.</span>
            <span>#2 Requires an external sidecar.</span>
            <span>#3 Work in progress.</span>
          </div>
        </div>
      </section>
    </>
  );
}
