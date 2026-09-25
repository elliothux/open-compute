import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import {
  IconBook,
  IconBrandGithubFilled,
  IconWorld,
} from "@tabler/icons-react";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { APIError } from "cloudflare/error";
import { useEffect, useState } from "react";
import { BrandLogo } from "../components/brand-logo";
import { CompatibilityMarquee } from "../components/compatibility-marquee";
import { openAlert } from "../components/dialog-manager";
import { useAuth } from "../features/auth/auth-atoms";
import {
  mintSessionFromAdmin,
  writeAuthSession,
} from "../features/auth/auth-session";
import { createManagementClient } from "../lib/cloudflare";

export const Route = createFileRoute("/login")({
  component: LoginPage,
});

const capabilityScenes = [
  "deploy",
  "worker-apis",
  "bindings",
  "extensions",
  "operate",
] as const;

type CapabilityScene = (typeof capabilityScenes)[number];

const sloganWords = [
  "for AI apps",
  "for agentic workflows",
  "for edge workloads",
  "for AI-native builders",
] as const;

function useTypewriter(
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

function LoginPage() {
  const { setToken, setInstanceId } = useAuth();
  const navigate = useNavigate();
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [scene] = useState<CapabilityScene>(
    () =>
      capabilityScenes[Math.floor(Math.random() * capabilityScenes.length)] ??
      "deploy",
  );
  const typedWord = useTypewriter(sloganWords);

  useEffect(() => {
    document.title = "open-compute";
  }, []);

  async function onSubmit(event: React.FormEvent) {
    event.preventDefault();
    setError(null);
    setPending(true);
    try {
      const trimmed = value.trim();
      if (!trimmed) {
        setError("Enter an admin token to continue.");
        return;
      }
      const session = await mintSessionFromAdmin(trimmed);
      const nextClient = createManagementClient(session.session_token);
      const accounts = await nextClient.accounts.list();
      const instance = accounts.result[0];
      if (instance?.id === undefined)
        throw new Error("No accessible instance was returned.");
      writeAuthSession(session.session_token, instance.id);
      setToken(session.session_token);
      setInstanceId(instance.id);
      openAlert({
        title: "This dashboard is in development",
        description:
          "You are viewing a development preview of the open-compute dashboard. Features and APIs may change without notice.",
        confirmText: "Got it",
      });
      await navigate({ to: "/" });
    } catch (caught) {
      setToken(null);
      if (caught instanceof APIError) {
        setError(caught.message);
      } else {
        setError("Unable to verify the admin token.");
      }
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="bg-kumo-canvas relative grid min-h-screen lg:grid-cols-[minmax(0,520px)_minmax(0,1fr)]">
      <section className="bg-kumo-base border-kumo-line flex w-full flex-col px-6 py-10 sm:px-10 lg:border-r lg:px-12">
        <div className="mx-auto w-full max-w-md">
          <BrandLogo variant="wordmark" />
        </div>
        <div className="mx-auto grid w-full max-w-md flex-1 content-center gap-7 py-10">
          <h1 className="text-xl font-semibold">Log in to open-compute</h1>
          <form className="grid gap-4" onSubmit={onSubmit}>
            <Input
              id="token"
              label="Admin token"
              type="password"
              autoComplete="off"
              value={value}
              onChange={(event) => setValue(event.target.value)}
              placeholder="Bearer token value"
              autoFocus
            />
            {error ? (
              <div
                className="bg-kumo-danger-tint text-kumo-danger rounded-lg px-3 py-2 text-sm"
                role="alert"
              >
                {error}
              </div>
            ) : null}
            <Button
              type="submit"
              variant="primary"
              className="w-full justify-center"
              disabled={pending}
            >
              {pending ? "Verifying…" : "Continue"}
            </Button>
          </form>
          <nav
            aria-label="open-compute resources"
            className="grid grid-cols-3 gap-2"
          >
            <a
              className="ring-kumo-line hover:bg-kumo-tint flex h-9 items-center justify-center gap-2 rounded-lg px-2 text-sm ring-1"
              href="https://github.com/elliothux/open-compute"
              target="_blank"
              rel="noreferrer"
            >
              <IconBrandGithubFilled size={16} />
              GitHub
            </a>
            <a
              className="ring-kumo-line hover:bg-kumo-tint flex h-9 items-center justify-center gap-2 rounded-lg px-2 text-sm ring-1"
              href="https://open-compute.dev"
              target="_blank"
              rel="noreferrer"
            >
              <IconWorld size={16} />
              Website
            </a>
            <a
              className="ring-kumo-line hover:bg-kumo-tint flex h-9 items-center justify-center gap-2 rounded-lg px-2 text-sm ring-1"
              href="https://open-compute.dev/docs/"
              target="_blank"
              rel="noreferrer"
            >
              <IconBook size={16} />
              Docs
            </a>
          </nav>
        </div>
        <div className="mx-auto w-full max-w-md">
          <CompatibilityMarquee />
        </div>
      </section>

      <section className="relative hidden overflow-hidden lg:flex lg:flex-col lg:justify-end">
        <img
          src={`${import.meta.env.BASE_URL}assets/capabilities/${scene}.webp`}
          alt=""
          className="absolute inset-0 h-full w-full object-cover"
          draggable={false}
        />
        <div aria-hidden="true" className="bg-login-overlay absolute inset-0" />
        <div className="relative z-10 grid gap-4 p-12 xl:p-16">
          <h2 className="text-[clamp(26px,2.6vw,44px)] leading-[1.15] font-normal text-white">
            The open-source cloud
            <span className="text-login-accent block pt-1 font-mono whitespace-nowrap">
              [{typedWord}
              <span className="login-slogan__caret" aria-hidden="true" />]
            </span>
            on your infrastructure.
          </h2>
          <p className="text-login-muted max-w-120 text-sm leading-snug">
            Run a complete Cloudflare Workers-compatible platform on your own
            hardware with one Rust-powered binary.
          </p>
        </div>
      </section>
    </div>
  );
}
