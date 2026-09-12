import { BenefitsSection } from "./components/benefits-section";
import { CapabilitiesSection } from "./components/capabilities-section";
import { ChangelogSection } from "./components/changelog-section";
import { HeroSection } from "./components/hero-section";
import { PricingSection } from "./components/pricing-section";
import { SiteFooter } from "./components/site-footer";
import { SiteHeader } from "./components/site-header";

export function App() {
  return (
    <>
      <SiteHeader />
      <main>
        <HeroSection />
        <CapabilitiesSection />
        <BenefitsSection />
        <PricingSection />
        <ChangelogSection />
      </main>
      <SiteFooter />
    </>
  );
}
