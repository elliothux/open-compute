import type { HomeMessages } from "../../i18n/home";
import { SectionMeta } from "../primitives";
import styles from "./styles.module.css";

const cards = [
  {
    number: "001",
    assets: [
      "/assets/benefit-runtime.webp",
      "/assets/benefit-runtime-ghost.webp",
    ],
  },
  {
    number: "002",
    assets: [
      "/assets/benefit-binary.webp",
      "/assets/benefit-binary-ghost.webp",
    ],
  },
  {
    number: "003",
    assets: [
      "/assets/benefit-storage.webp",
      "/assets/benefit-storage-ghost.webp",
    ],
  },
] as const;

export function BenefitsSection({
  messages,
}: {
  messages: HomeMessages["benefits"];
}) {
  return (
    <section className={`benefits ${styles.module}`} id="architecture">
      <div className="section-shell">
        <SectionMeta index="03" label={messages.label} />
        <div className="benefits__heading">
          <h2 className="display-heading">{messages.heading}</h2>
        </div>
        <div className="benefits__grid">
          {cards.map((card, cardIndex) => {
            const copy = messages.cards[cardIndex];
            if (!copy) throw new Error(`Missing benefit copy at ${cardIndex}.`);
            return (
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
                    <h3>{copy.title}</h3>
                    <div className="benefit-card__description">
                      <p>{copy.description}</p>
                    </div>
                  </div>
                </div>
                <span className="benefit-card__caption mono">
                  {copy.caption}
                </span>
              </article>
            );
          })}
        </div>
      </div>
    </section>
  );
}
