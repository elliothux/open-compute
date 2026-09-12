import { useEffect, useState } from "react";
import { PixelArrowTopRight } from "../icons";
import { ScrambleLabel, useScrambleText } from "../primitives";
import styles from "./styles.module.css";

const links = [
  ["OPEN-COMPUTE", "#top"],
  ["DEVELOP", "/docs/develop/"],
  ["OPERATE", "/docs/operate/"],
  ["PRODUCTS", "/docs/products/"],
  ["REFERENCE", "/docs/reference/"],
  ["GITHUB", "https://github.com/elliothux/open-compute"],
] as const;

export function SiteHeader() {
  const [menuOpen, setMenuOpen] = useState(false);
  const [activeHref, setActiveHref] = useState("#top");

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
  }, []);

  return (
    <header
      className={`${menuOpen ? "site-header is-menu-open" : "site-header"} ${styles.module}`}
    >
      <nav className="site-header__desktop" aria-label="Primary navigation">
        <div className="site-header__links">
          {links.map(([label, href]) => (
            <NavLink
              href={href}
              active={href === activeHref}
              key={label}
              label={label}
            />
          ))}
        </div>
        <ContactLink />
      </nav>
      <nav className="site-header__mobile" aria-label="Mobile navigation">
        <a className="site-header__brand" href="#top">
          <img src="/favicon.svg" alt="" /> <span>open-compute</span>
        </a>
        <button
          className="site-header__menu"
          type="button"
          aria-label={menuOpen ? "Close navigation" : "Open navigation"}
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
      </div>
    </header>
  );
}

function NavLink({
  active,
  href,
  label,
}: {
  active: boolean;
  href: string;
  label: string;
}) {
  const [scrambled, scramble] = useScrambleText(label);

  return (
    <a
      className={active ? "is-active" : undefined}
      href={href}
      onFocus={active ? undefined : scramble}
      onMouseEnter={active ? undefined : scramble}
    >
      <ScrambleLabel value={`<${label}>`}>
        {active ? `<${label}>` : scrambled}
      </ScrambleLabel>
    </a>
  );
}

function ContactLink() {
  const [label, scramble] = useScrambleText("GET STARTED");

  return (
    <a
      className="site-header__contact"
      href="/docs/get-started/"
      onFocus={scramble}
      onMouseEnter={scramble}
    >
      <span className="site-header__contact-icon">
        <PixelArrowTopRight />
        <PixelArrowTopRight />
      </span>
      <span className="site-header__contact-label">
        <ScrambleLabel value="GET STARTED">{label}</ScrambleLabel>
      </span>
    </a>
  );
}
