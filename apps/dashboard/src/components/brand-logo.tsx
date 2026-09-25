import { useTheme } from "../features/theme/theme-atoms";

type BrandLogoVariant = "wordmark" | "mark";

interface BrandLogoProps {
  variant?: BrandLogoVariant;
  className?: string;
}

const brandBase = `${import.meta.env.BASE_URL}brand`;

export function BrandLogo({ variant = "wordmark", className }: BrandLogoProps) {
  const { resolved } = useTheme();
  const tone = resolved === "dark" ? "white" : "black";

  if (variant === "wordmark") {
    return (
      <span
        className={["inline-flex items-center gap-2.5", className]
          .filter(Boolean)
          .join(" ")}
      >
        <img
          src={`${brandBase}/logo-${tone}.svg`}
          alt=""
          className="size-7 shrink-0"
          draggable={false}
        />
        <img
          src={`${brandBase}/logo-text-${tone}.svg`}
          alt="open-compute"
          className="h-7 w-auto"
          draggable={false}
        />
      </span>
    );
  }

  return (
    <img
      src={`${brandBase}/logo-${tone}.svg`}
      alt="open-compute"
      className={className ?? "size-8"}
      draggable={false}
    />
  );
}
