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

const sloganWords = [
  "for AI workloads",
  "for APIs",
  "for full-stack apps",
] as const;

const links = [
  ["Get started", "/docs/get-started/"],
  ["Develop", "/docs/develop/"],
  ["Operate", "/docs/operate/"],
  ["Products", "/docs/products/"],
  ["Reference", "/docs/reference/"],
  ["GitHub", "https://github.com/elliothux/open-compute"],
] as const;

export function SiteFooter() {
  const typedWord = useTypewriter(sloganWords, 100, 1000, 60);

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
          <span>The open-source cloud</span>
          <span className="footer__typed">
            [{typedWord}
            <span className="type-caret" />]
          </span>
          <span>on your infrastructure.</span>
        </h2>
        <div className="footer__actions">
          <ActionButton href="/docs/get-started/">INSTALL</ActionButton>
          <FooterGitHubButton />
        </div>
        <nav className="footer__links" aria-label="Footer navigation">
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

function FooterGitHubButton() {
  const [label, scramble] = useScrambleText("GITHUB");

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
      <ScrambleLabel value="GITHUB">{label}</ScrambleLabel>
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
