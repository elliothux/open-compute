import { SectionMeta } from "../primitives";
import styles from "./styles.module.css";

const cards = [
  {
    number: "001",
    title: "Isolates, not containers.",
    description: "Native workerd isolation without a container per request.",
    caption: "ISOLATION",
    assets: [
      "/assets/benefit-runtime.webp",
      "/assets/benefit-runtime-ghost.webp",
    ],
  },
  {
    number: "002",
    title: "One binary. No sidecars.",
    description:
      "Runtime, control plane, storage integration, and Dashboard ship together.",
    caption: "OPERATIONS",
    assets: [
      "/assets/benefit-binary.webp",
      "/assets/benefit-binary-ghost.webp",
    ],
  },
  {
    number: "003",
    title: "Authority stays local.",
    description:
      "SQLite is authoritative. Deployments are immutable. Artifacts are content-addressed.",
    caption: "STATE",
    assets: [
      "/assets/benefit-storage.webp",
      "/assets/benefit-storage-ghost.webp",
    ],
  },
] as const;

export function BenefitsSection() {
  return (
    <section className={`benefits ${styles.module}`} id="architecture">
      <div className="section-shell">
        <SectionMeta index="03" label="ARCHITECTURE" />
        <div className="benefits__heading">
          <h2 className="display-heading">
            A compact stack for self-hosted Workers.
          </h2>
        </div>
        <div className="benefits__grid">
          {cards.map((card) => (
            <article className="benefit-card reveal-card" key={card.number}>
              <div className="benefit-card__top">
                <div className="benefit-card__visual">
                  {card.assets.map((asset, index) => (
                    <img
                      className={
                        index === 1 ? "benefit-card__ghost" : undefined
                      }
                      src={asset}
                      alt=""
                      key={asset}
                    />
                  ))}
                  <span className="benefit-card__number">
                    <i>//</i>
                    <b>{card.number}</b>
                  </span>
                </div>
                <div className="benefit-card__content">
                  <h3>{card.title}</h3>
                  <div className="benefit-card__description">
                    <p>{card.description}</p>
                  </div>
                </div>
              </div>
              <span className="benefit-card__caption mono">{card.caption}</span>
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}
