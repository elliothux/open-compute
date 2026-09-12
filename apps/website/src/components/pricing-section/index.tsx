import { PricingCheckIcon, PricingProIcon } from "../icons";
import { ActionButton, SectionMeta } from "../primitives";
import styles from "./styles.module.css";

const plans = [
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
    popular: false,
    disabled: false,
    action: "GET STARTED",
    href: "/docs/get-started/",
  },
  {
    name: "Managed",
    badge: "COMING SOON",
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
    popular: false,
    disabled: true,
    action: "COMING SOON",
    href: "/#pricing",
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
    popular: true,
    disabled: false,
    action: "CONTACT US",
    href: "mailto:elliothu.my@gmail.com?subject=open-compute%20Enterprise",
  },
] as const;

export function PricingSection() {
  return (
    <section className={`pricing ${styles.module}`} id="pricing">
      <div className="section-shell">
        <SectionMeta index="04" label="PRICING" dark />
        <div className="section-heading-row pricing__heading">
          <div>
            <h2 className="display-heading">Self-host for free.</h2>
          </div>
        </div>
        <div className="pricing__cards">
          {plans.map((plan) => (
            <article
              className={plan.popular ? "price-card is-popular" : "price-card"}
              key={plan.name}
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
                      <h3>{plan.name}</h3>
                      {plan.badge && (
                        <span className="price-card__badge">{plan.badge}</span>
                      )}
                    </div>
                    <p>{plan.description}</p>
                  </div>
                </div>
                <div className="price-card__price">
                  <strong>{plan.price}</strong>
                  <span>{plan.suffix}</span>
                </div>
                <ActionButton href={plan.href} disabled={plan.disabled}>
                  {plan.action}
                </ActionButton>
              </div>
              <div className="price-card__features">
                <strong>{plan.intro}</strong>
                <ul>
                  {plan.features.map((feature) => (
                    <li key={feature}>
                      <PricingCheckIcon /> {feature}
                    </li>
                  ))}
                </ul>
              </div>
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}
