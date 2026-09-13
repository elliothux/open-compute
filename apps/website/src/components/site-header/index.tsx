import { useEffect, useMemo, useState } from "react";
import { docsHref, homeHref, type Locale } from "../../i18n/config";
import type { HomeMessages } from "../../i18n/home";
import { PixelArrowTopRight } from "../icons";
import { ScrambleLabel, useScrambleText } from "../primitives";
import styles from "./styles.module.css";

type NavigationMessages = HomeMessages["navigation"];

export function SiteHeader({
  alternateLocaleHref,
  locale,
  messages,
}: {
  alternateLocaleHref: string;
  locale: Locale;
  messages: NavigationMessages;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [activeHref, setActiveHref] = useState("#top");
  const links = useMemo(
    () =>
      [
        [messages.links.home, "#top"],
        [messages.links.develop, docsHref(locale, "develop")],
        [messages.links.operate, docsHref(locale, "operate")],
        [messages.links.products, docsHref(locale, "products")],
        [messages.links.github, "https://github.com/elliothux/open-compute"],
      ] as const,
    [locale, messages],
  );

  useEffect(() => {
    let frame = 0;
    const sectionLinks = links.filter(([, href]) => href.startsWith("#"));

    const updateActiveLink = () => {
      window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(() => {
        const marker = window.scrollY + window.innerHeight * 0.35;
        let nextHref = sectionLinks[0]?.[1] ?? "#top";

        for (const [, href] of sectionLinks) {
          const section = document.querySelector<HTMLElement>(href);
          if (section && section.offsetTop <= marker) nextHref = href;
        }

        setActiveHref(nextHref);
      });
    };

    updateActiveLink();
    window.addEventListener("scroll", updateActiveLink, { passive: true });
    window.addEventListener("resize", updateActiveLink);

    return () => {
      window.cancelAnimationFrame(frame);
      window.removeEventListener("scroll", updateActiveLink);
      window.removeEventListener("resize", updateActiveLink);
    };
  }, [links]);

  return (
    <header
      className={`${menuOpen ? "site-header is-menu-open" : "site-header"} ${styles.module}`}
    >
      <nav className="site-header__desktop" aria-label={messages.ariaLabel}>
        <div className="site-header__links">
          {links.map(([label, href]) => (
            <NavLink
              href={href}
              active={href === activeHref}
              key={label}
              label={label}
            />
          ))}
          <NavLink
            active={false}
            href={alternateLocaleHref}
            hrefLang={locale === "en" ? "zh-CN" : "en"}
            label={messages.language}
          />
        </div>
        <ContactLink
          href={docsHref(locale, "get-started")}
          label={messages.contact}
        />
      </nav>
      <nav
        className="site-header__mobile"
        aria-label={messages.mobileAriaLabel}
      >
        <a className="site-header__brand" href={homeHref(locale)}>
          <img src="/favicon.svg" alt="" /> <span>open-compute</span>
        </a>
        <button
          className="site-header__menu"
          type="button"
          aria-label={menuOpen ? messages.close : messages.open}
          aria-expanded={menuOpen}
          onClick={() => setMenuOpen((current) => !current)}
        >
          <span />
          <span />
        </button>
      </nav>
      <div className="site-header__mobile-panel">
        {links.map(([label, href], index) => (
          <a href={href} onClick={() => setMenuOpen(false)} key={label}>
            <span className="mono">
              // {String(index + 1).padStart(2, "0")}
            </span>
            <strong>{label}</strong>
            <PixelArrowTopRight />
          </a>
        ))}
        <a href={alternateLocaleHref} onClick={() => setMenuOpen(false)}>
          <span className="mono">
            // {String(links.length + 1).padStart(2, "0")}
          </span>
          <strong>{messages.language}</strong>
          <PixelArrowTopRight />
        </a>
      </div>
    </header>
  );
}

function NavLink({
  active,
  href,
  hrefLang,
  label,
}: {
  active: boolean;
  href: string;
  hrefLang?: string;
  label: string;
}) {
  const [scrambled, scramble] = useScrambleText(label);

  return (
    <a
      className={active ? "is-active" : undefined}
      href={href}
      hrefLang={hrefLang}
      onFocus={active ? undefined : scramble}
      onMouseEnter={active ? undefined : scramble}
    >
      <ScrambleLabel value={`<${label}>`}>
        {active ? `<${label}>` : scrambled}
      </ScrambleLabel>
    </a>
  );
}

function ContactLink({ href, label: value }: { href: string; label: string }) {
  const [label, scramble] = useScrambleText(value);

  return (
    <a
      className="site-header__contact"
      href={href}
      onFocus={scramble}
      onMouseEnter={scramble}
    >
      <span className="site-header__contact-icon">
        <PixelArrowTopRight />
        <PixelArrowTopRight />
      </span>
      <span className="site-header__contact-label">
        <ScrambleLabel value={value}>{label}</ScrambleLabel>
      </span>
    </a>
  );
}
