import { useEffect, useState } from "react";
import { BracketsAngleIcon, CheckDoubleIcon } from "../icons";
import { ActionButton, SectionMeta } from "../primitives";
import styles from "./styles.module.css";

interface Release {
  publishedAt: string;
  summary: string;
  tagName: string;
  url: string;
}

const fallbackReleases: readonly Release[] = [
  {
    publishedAt: "2026-09-10T09:53:01Z",
    summary:
      "0.1.4 adds a complete Cloudflare Artifacts implementation for self-hosted open-compute installations. Operators can create namespaces and repositories, use Wrangler and Worker bindings, push and clone through Git Smart HTTP, and include repository data in the existing snapshot and restore lifecycle.",
    tagName: "0.1.4",
    url: "https://github.com/elliothux/open-compute/releases",
  },
  {
    publishedAt: "2026-09-09T13:57:41Z",
    summary:
      "0.1.3 adds a project-native Wrangler workflow for teams running Cloudflare-compatible Workers on a self-hosted open-compute instance. A repository can now keep an ordinary wrangler.jsonc, select a named open-compute target, and use pinned Wrangler commands without copying control-plane URLs or credentials into project files.",
    tagName: "0.1.3",
    url: "https://github.com/elliothux/open-compute/releases",
  },
  {
    publishedAt: "2026-09-08T05:59:16Z",
    summary:
      "0.1.2 is the first release with a complete day-to-day operator workflow for a single-machine open-compute installation. You can now initialize an instance, run it as an OS service, inspect it, open its Dashboard, upgrade it, and uninstall it safely.",
    tagName: "0.1.2",
    url: "https://github.com/elliothux/open-compute/releases",
  },
];

function isRelease(value: unknown): value is Release {
  if (typeof value !== "object" || value === null) return false;

  return (
    "publishedAt" in value &&
    typeof value.publishedAt === "string" &&
    "summary" in value &&
    typeof value.summary === "string" &&
    "tagName" in value &&
    typeof value.tagName === "string" &&
    "url" in value &&
    typeof value.url === "string"
  );
}

function formatReleaseDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "RELEASED";

  return new Intl.DateTimeFormat("en-US", {
    day: "2-digit",
    month: "short",
    timeZone: "UTC",
    year: "numeric",
  })
    .format(date)
    .toUpperCase();
}

export function ChangelogSection() {
  const [releases, setReleases] =
    useState<readonly Release[]>(fallbackReleases);

  useEffect(() => {
    const controller = new AbortController();

    void fetch("/api/releases/?content=body-v1", {
      headers: { accept: "application/json" },
      signal: controller.signal,
    })
      .then(async (response) => {
        if (!response.ok) throw new Error("Releases unavailable");
        return (await response.json()) as unknown;
      })
      .then((payload) => {
        if (
          typeof payload !== "object" ||
          payload === null ||
          !("releases" in payload) ||
          !Array.isArray(payload.releases) ||
          payload.releases.length === 0 ||
          !payload.releases.every(isRelease)
        ) {
          return;
        }
        setReleases(payload.releases.slice(0, 3));
      })
      .catch(() => {
        // Keep the server-rendered fallback when GitHub is unavailable.
      });

    return () => controller.abort();
  }, []);

  return (
    <section className={`changelog ${styles.module}`} id="changelog">
      <div className="section-shell">
        <SectionMeta index="05" label="RECENT RELEASES" />
        <div className="changelog__main">
          <div className="changelog__left">
            <div>
              <h2 className="display-heading">Recent releases.</h2>
            </div>
            <div className="changelog__copy">
              <ActionButton href="https://github.com/elliothux/open-compute/releases">
                VIEW ALL
              </ActionButton>
            </div>
          </div>
          <div className="changelog__right">
            <div className="changelog__list">
              {releases.map((release, index) => {
                const date = formatReleaseDate(release.publishedAt);
                return (
                  <div className="changelog__item" key={release.tagName}>
                    <div className="changelog__line changelog__line--title">
                      <span className="changelog__space" />
                      <div className="changelog__title">
                        <span className="changelog__event-icon">
                          {index === 0 ? (
                            <BracketsAngleIcon />
                          ) : (
                            <CheckDoubleIcon />
                          )}
                        </span>
                        <span className="changelog__mobile-date mono">
                          {date}
                        </span>
                        <h3>
                          <a href={release.url}>{release.tagName}</a>
                        </h3>
                      </div>
                    </div>
                    <div className="changelog__gap">
                      <span className="changelog__date-frame">
                        <span className="changelog__date mono">{date}</span>
                      </span>
                      <span className="changelog__dash" />
                    </div>
                    <div className="changelog__line changelog__line--description">
                      <span className="changelog__space" />
                      <div className="changelog__description">
                        <p>{release.summary}</p>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
            <div className="changelog__center" aria-hidden="true">
              <span />
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}
