import { Button } from "@cloudflare/kumo/components/button";
import { IconCheck, IconCopy } from "@tabler/icons-react";
import { common, createStarryNight, type Options } from "@wooorm/starry-night";
import { toHtml } from "hast-util-to-html";
import { useEffect, useRef, useState } from "react";
import onigurumaWasmUrl from "vscode-oniguruma/release/onig.wasm?url";

type StarryNight = Awaited<ReturnType<typeof createStarryNight>>;

const scopeFlags = {
  bash: "bash",
  javascript: "js",
  json: "json",
  typescript: "ts",
} as const;

export type CodeLanguage = keyof typeof scopeFlags;

let starryNightPromise: Promise<StarryNight> | undefined;

function loadStarryNight(): Promise<StarryNight> {
  // Same-origin wasm asset bundled from the pinned vscode-oniguruma release;
  // the default loader would fetch it from a third-party CDN at runtime.
  starryNightPromise ??= createStarryNight(common, {
    getOnigurumaUrlFetch: () =>
      Promise.resolve(new URL(onigurumaWasmUrl, window.location.origin)),
  } satisfies Options);
  return starryNightPromise;
}

/**
 * Code block highlighted with VS Code TextMate grammars, styled through the
 * GitHub prettylights theme variables in app.css. Renders plain text until
 * the grammar engine finishes loading. With `copy`, a button in the corner
 * copies the raw code to the clipboard.
 */
export function CodeBlock({
  className,
  code,
  copy = false,
  language,
}: {
  className?: string | undefined;
  code: string;
  copy?: boolean;
  language: CodeLanguage;
}) {
  const [html, setHtml] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const resetTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    let active = true;
    loadStarryNight()
      .then((starryNight) => {
        if (!active) return;
        const scope = starryNight.flagToScope(scopeFlags[language]);
        if (!scope) return;
        setHtml(toHtml(starryNight.highlight(code, scope)));
      })
      .catch(() => {});
    return () => {
      active = false;
      window.clearTimeout(resetTimer.current);
    };
  }, [code, language]);

  async function copyCode() {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      window.clearTimeout(resetTimer.current);
      resetTimer.current = window.setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  }

  const body =
    html === null ? (
      <code>{code}</code>
    ) : (
      <code dangerouslySetInnerHTML={{ __html: html }} />
    );

  if (!copy) {
    return <pre className={className}>{body}</pre>;
  }
  return (
    <div className="relative min-w-0">
      <pre className={className}>{body}</pre>
      <Button
        aria-label={copied ? "Copied" : "Copy code"}
        className="absolute top-1/2 right-2 -translate-y-1/2"
        disabled={copied}
        shape="square"
        size="xs"
        variant="secondary"
        onClick={() => void copyCode()}
      >
        {copied ? <IconCheck size={14} /> : <IconCopy size={14} />}
      </Button>
    </div>
  );
}
