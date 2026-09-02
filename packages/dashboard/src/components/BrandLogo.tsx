import { useTheme } from "../features/theme/ThemeProvider";

type BrandLogoVariant = "wordmark" | "mark";

interface BrandLogoProps {
  variant?: BrandLogoVariant;
  className?: string;
}

const brandBase = `${import.meta.env.BASE_URL}brand`;

export function BrandLogo({ variant = "wordmark", className }: BrandLogoProps) {
  const { resolved } = useTheme();
  const tone = resolved === "dark" ? "white" : "black";
  const src = `${brandBase}/logo-${tone}.svg`;

  if (variant === "wordmark") {
    return (
      <span className={["inline-flex items-center gap-2", className].filter(Boolean).join(" ")}>
        <img src={src} alt="" className="size-7 shrink-0" draggable={false} />
        <span className="whitespace-nowrap text-base font-semibold text-kumo-default">open-compute</span>
      </span>
    );
  }

  return (
    <img
      src={src}
      alt="open-compute"
      className={className ?? "size-8"}
      draggable={false}
    />
  );
}
