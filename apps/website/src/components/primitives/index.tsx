import { useCallback, useEffect, useRef, useState } from "react";
import { PixelArrowRight } from "../icons";
import styles from "./styles.module.css";

export function SectionMeta({
  index,
  label,
  dark = false,
  total = "05",
}: {
  index: string;
  label: string;
  dark?: boolean;
  total?: string;
}) {
  return (
    <div
      className={`${dark ? "section-meta section-meta--dark" : "section-meta"} ${styles.module}`}
    >
      <span>
        [N.{index}/{total}]
      </span>
      <span className="section-meta__dash">—</span>
      <span>&gt; {label}</span>
    </div>
  );
}

export function ActionButton({
  children,
  href = "#footer",
  disabled = false,
}: {
  children: string;
  href?: string;
  disabled?: boolean;
}) {
  const [label, scramble] = useScrambleText(children);

  return (
    <a
      aria-disabled={disabled || undefined}
      className={`action-button${disabled ? " is-disabled" : ""} ${styles.module}`}
      href={disabled ? undefined : href}
      onMouseEnter={disabled ? undefined : scramble}
      onFocus={disabled ? undefined : scramble}
    >
      <span className="action-dot" />
      <span className="action-label">
        <ScrambleLabel value={children}>{label}</ScrambleLabel>
      </span>
      <span className="action-button__icon-wrap">
        <PixelArrowRight className="action-button__icon" />
      </span>
      <i className="action-button__background" />
    </a>
  );
}

export function ScrambleLabel({
  children,
  value,
}: {
  children: React.ReactNode;
  value: string;
}) {
  return (
    <span className="scramble-label" aria-label={value}>
      <span className="scramble-label__sizer" aria-hidden="true">
        {value}
      </span>
      <span className="scramble-label__value" aria-hidden="true">
        {children}
      </span>
    </span>
  );
}

const SCRAMBLE_MAX_DURATION_MS = 720;

export function useScrambleText(value: string) {
  const [animatedLabel, setAnimatedLabel] = useState({
    source: value,
    text: value,
  });
  const timer = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (timer.current !== null) window.clearInterval(timer.current);
    };
  }, []);

  const scramble = useCallback(() => {
    if (timer.current !== null) window.clearInterval(timer.current);
    const glyphs = "!<>-_\\/[]{}—=+*^?#";
    const characters = [...value];
    const duration = Math.min(
      SCRAMBLE_MAX_DURATION_MS,
      Math.max(240, characters.length * 42),
    );
    const startedAt = performance.now();

    timer.current = window.setInterval(() => {
      const progress = Math.min(1, (performance.now() - startedAt) / duration);
      const revealed = Math.floor(progress * characters.length);
      setAnimatedLabel({
        source: value,
        text: characters
          .map((character, index) => {
            if (character === " ") return " ";
            if (index < revealed) return character;
            return (
              glyphs[Math.floor(Math.random() * glyphs.length)] ?? character
            );
          })
          .join(""),
      });
      if (progress >= 1) {
        if (timer.current !== null) window.clearInterval(timer.current);
        timer.current = null;
        setAnimatedLabel({ source: value, text: value });
      }
    }, 30);
  }, [value]);

  const label = animatedLabel.source === value ? animatedLabel.text : value;
  return [label, scramble] as const;
}

export function useTypewriter(
  words: readonly string[],
  typeDelay = 100,
  pauseDelay = 1000,
  deleteDelay = 60,
) {
  const [wordIndex, setWordIndex] = useState(0);
  const [visibleLength, setVisibleLength] = useState(0);
  const [erasing, setErasing] = useState(false);

  useEffect(() => {
    const word = words[wordIndex] ?? "";
    const atEdge = erasing
      ? visibleLength === 0
      : visibleLength === word.length;
    const delay =
      atEdge && !erasing ? pauseDelay : erasing ? deleteDelay : typeDelay;
    const timer = window.setTimeout(() => {
      if (!erasing && visibleLength >= word.length) {
        setErasing(true);
      } else if (erasing && visibleLength <= 0) {
        setErasing(false);
        setWordIndex((current) => (current + 1) % words.length);
      } else {
        setVisibleLength((current) => current + (erasing ? -1 : 1));
      }
    }, delay);
    return () => window.clearTimeout(timer);
  }, [
    deleteDelay,
    erasing,
    pauseDelay,
    typeDelay,
    visibleLength,
    wordIndex,
    words,
  ]);

  return words[wordIndex]?.slice(0, visibleLength) ?? "";
}
