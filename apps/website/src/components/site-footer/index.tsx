import { docsHref, type Locale } from "../../i18n/config";
import type { HomeMessages } from "../../i18n/home";
import { PixelArrowTopRight } from "../icons";
import {
  ActionButton,
  ScrambleLabel,
  useScrambleText,
  useTypewriter,
} from "../primitives";
import styles from "./styles.module.css";

const footerVideoUrl =
  "https://static.open-compute.dev/videos/open-compute-footer-9d4d170ff977.mp4";

export function SiteFooter({
  locale,
  messages,
}: {
  locale: Locale;
  messages: HomeMessages["footer"];
}) {
  const typedWord = useTypewriter(messages.sloganWords, 100, 1000, 60);
  const links = [
    [messages.links.getStarted, docsHref(locale, "get-started")],
    [messages.links.develop, docsHref(locale, "develop")],
    [messages.links.operate, docsHref(locale, "operate")],
    [messages.links.products, docsHref(locale, "products")],
    [messages.links.reference, docsHref(locale, "reference")],
    [messages.links.github, "https://github.com/elliothux/open-compute"],
  ] as const;

  return (
    <footer className={`footer ${styles.module}`} id="footer">
      <div className="footer__background" aria-hidden="true">
        <video autoPlay className="footer__video" loop muted playsInline>
          <source src={footerVideoUrl} type="video/mp4" />
        </video>
      </div>
      <div className="footer__veil" aria-hidden="true" />
      <div className="section-shell footer__inner">
        <h2 className="footer__slogan">
          <span>{messages.sloganPrefix}</span>
          <span className="footer__typed">
            [{typedWord}
            <span className="type-caret" />]
          </span>
          <span>{messages.sloganSuffix}</span>
        </h2>
        <div className="footer__actions">
          <ActionButton href={docsHref(locale, "get-started")}>
            {messages.install}
          </ActionButton>
          <FooterGitHubButton label={messages.links.github.toUpperCase()} />
        </div>
        <nav className="footer__links" aria-label={messages.ariaLabel}>
          {links.map(([label, href]) => (
            <FooterLink href={href} label={label} key={label} />
          ))}
          <FooterLink
            className="footer__email"
            href="mailto:elliothu.my@gmail.com?subject=open-compute%20Enterprise"
            label="elliothu.my@gmail.com"
          />
        </nav>
      </div>
    </footer>
  );
}

function FooterGitHubButton({ label: value }: { label: string }) {
  const [label, scramble] = useScrambleText(value);

  return (
    <a
      className="footer__github"
      href="https://github.com/elliothux/open-compute"
      onFocus={scramble}
      onMouseEnter={scramble}
    >
      <span className="footer__github-icon">
        <PixelArrowTopRight />
      </span>
      <ScrambleLabel value={value}>{label}</ScrambleLabel>
    </a>
  );
}

function FooterLink({
  className,
  href,
  label,
}: {
  className?: string;
  href: string;
  label: string;
}) {
  const [scrambledLabel, scrambleLabel] = useScrambleText(label);

  return (
    <a
      className={className}
      href={href}
      onFocus={scrambleLabel}
      onMouseEnter={scrambleLabel}
    >
      <ScrambleLabel value={label}>{scrambledLabel}</ScrambleLabel>
    </a>
  );
}
