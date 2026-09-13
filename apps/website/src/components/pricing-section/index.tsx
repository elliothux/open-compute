import { docsHref, type Locale } from "../../i18n/config";
import type { HomeMessages } from "../../i18n/home";
import { PricingCheckIcon, PricingProIcon } from "../icons";
import { ActionButton, SectionMeta } from "../primitives";
import styles from "./styles.module.css";

const plans = [
  {
    popular: false,
    disabled: false,
    href: "docs",
  },
  {
    popular: false,
    disabled: true,
    href: "pricing",
  },
  {
    popular: true,
    disabled: false,
    href: "contact",
  },
] as const;

export function PricingSection({
  locale,
  messages,
}: {
  locale: Locale;
  messages: HomeMessages["pricing"];
}) {
  return (
    <section className={`pricing ${styles.module}`} id="pricing">
      <div className="section-shell">
        <SectionMeta index="04" label={messages.label} dark />
        <div className="section-heading-row pricing__heading">
          <div>
            <h2 className="display-heading">{messages.heading}</h2>
          </div>
        </div>
        <div className="pricing__cards">
          {plans.map((plan, planIndex) => {
            const copy = messages.plans[planIndex];
            if (!copy) throw new Error(`Missing pricing copy at ${planIndex}.`);
            const href =
              plan.href === "docs"
                ? docsHref(locale, "get-started")
                : plan.href === "pricing"
                  ? "#pricing"
                  : "mailto:elliothu.my@gmail.com?subject=open-compute%20Enterprise";
            return (
              <article
                className={
                  plan.popular ? "price-card is-popular" : "price-card"
                }
                key={copy.name}
              >
                {plan.popular && (
                  <span className="price-card__outline" aria-hidden="true">
                    <i />
                    <i />
                    <i />
                    <i />
                  </span>
                )}
                <div className="price-card__top">
                  <div className="price-card__header">
                    <div className="price-card__title">
                      <div className="price-card__name">
                        {plan.popular && <PricingProIcon />}
                        <h3>{copy.name}</h3>
                        {copy.badge && (
                          <span className="price-card__badge">
                            {copy.badge}
                          </span>
                        )}
                      </div>
                      <p>{copy.description}</p>
                    </div>
                  </div>
                  <div className="price-card__price">
                    <strong>{copy.price}</strong>
                    <span>{copy.suffix}</span>
                  </div>
                  <ActionButton href={href} disabled={plan.disabled}>
                    {copy.action}
                  </ActionButton>
                </div>
                <div className="price-card__features">
                  <strong>{copy.intro}</strong>
                  <ul>
                    {copy.features.map((feature) => (
                      <li key={feature}>
                        <PricingCheckIcon /> {feature}
                      </li>
                    ))}
                  </ul>
                </div>
              </article>
            );
          })}
        </div>
      </div>
    </section>
  );
}
