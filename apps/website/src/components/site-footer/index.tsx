import { PixelArrowTopRight } from "../icons";
import {
  ActionButton,
  ScrambleLabel,
  useScrambleText,
  useTypewriter,
} from "../primitives";
import { BeamsBackground } from "./beams-background";
import styles from "./styles.module.css";

const sloganWords = [
  "for AI workloads",
  "for APIs",
  "for full-stack apps",
] as const;

const links = [
  ["Docs", "/docs/"],
  ["Compatibility", "/docs/platform/compatibility/"],
  ["Pricing", "#pricing"],
  ["GitHub", "https://github.com/elliothux/open-compute"],
] as const;

export function SiteFooter() {
  const typedWord = useTypewriter(sloganWords, 100, 1000, 60);

  return (
    <footer className={`footer ${styles.module}`} id="footer">
      <BeamsBackground
        beamWidth={1.2}
        beamHeight={13}
        beamNumber={32}
        lightColor="#ffffff"
        speed={5}
        noiseIntensity={3.9}
        scale={0.2}
        rotation={30}
        beamColor="#000000"
        backgroundColor="#000000"
      />
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
