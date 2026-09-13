import { BenefitsSection } from "./components/benefits-section";
import { CapabilitiesSection } from "./components/capabilities-section";
import { ChangelogSection } from "./components/changelog-section";
import { HeroSection } from "./components/hero-section";
import { PricingSection } from "./components/pricing-section";
import { SiteFooter } from "./components/site-footer";
import { SiteHeader } from "./components/site-header";
import type { Locale } from "./i18n/config";
import type { HomeMessages } from "./i18n/home";

interface Props {
  alternateLocaleHref: string;
  locale: Locale;
  messages: HomeMessages;
}

export function App({ alternateLocaleHref, locale, messages }: Props) {
  return (
    <>
      <SiteHeader
        alternateLocaleHref={alternateLocaleHref}
        locale={locale}
        messages={messages.navigation}
      />
      <main>
        <HeroSection locale={locale} messages={messages.hero} />
        <CapabilitiesSection messages={messages.capabilities} />
        <BenefitsSection messages={messages.benefits} />
        <PricingSection locale={locale} messages={messages.pricing} />
        <ChangelogSection locale={locale} messages={messages.changelog} />
      </main>
      <SiteFooter locale={locale} messages={messages.footer} />
    </>
  );
}
