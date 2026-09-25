import { Button } from "@cloudflare/kumo/components/button";
import { IconX } from "@tabler/icons-react";
import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { createPortal } from "react-dom";
import { BrandLogo } from "./brand-logo";

export function CreateStepper({
  title,
  steps,
  current,
  onClose,
  children,
}: {
  title: string;
  steps: string[];
  current: number;
  onClose: () => void;
  children: ReactNode;
}) {
  return createPortal(
    <div className="bg-kumo-canvas fixed inset-0 z-50 overflow-y-auto">
      <header className="border-kumo-line bg-kumo-base flex h-16 items-center justify-between border-b px-5">
        <Link
          to="/"
          aria-label="Account home"
          className="flex items-center gap-2 font-medium"
        >
          <BrandLogo variant="mark" className="size-7" />
          <span>open-compute</span>
        </Link>
        <Button
          variant="ghost"
          shape="square"
          aria-label="Close"
          onClick={onClose}
        >
          <IconX size={18} />
        </Button>
      </header>
      <main className="mx-auto grid max-w-7xl content-start gap-6 px-5 py-8 md:grid-cols-4 md:content-normal md:gap-7 md:px-8 md:py-14">
        <div className="border-kumo-line order-1 md:border-r md:border-dotted md:pr-7">
          <h1 className="font-semibold">{title}</h1>
        </div>
        <div className="order-3 min-w-0 md:order-2 md:col-span-2">
          {children}
        </div>
        <aside
          className="border-kumo-line order-2 md:order-3 md:border-l md:border-dotted md:pl-7"
          aria-label="Create progress"
        >
          <ol className="grid gap-3 text-sm">
            {steps.map((step, index) => (
              <li
                key={step}
                className={
                  index === current ? "font-semibold" : "text-kumo-subtle"
                }
              >
                {index === current ? "●" : "○"} {step}
              </li>
            ))}
          </ol>
        </aside>
      </main>
    </div>,
    document.body,
  );
}
