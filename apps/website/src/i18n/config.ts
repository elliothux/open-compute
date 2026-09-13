const locales = ["en", "zh"] as const;

export type Locale = (typeof locales)[number];

const defaultLocale: Locale = "en";

export const localeConfig = {
  en: {
    htmlLang: "en",
    label: "English",
    path: undefined,
  },
  zh: {
    htmlLang: "zh-CN",
    label: "简体中文",
    path: "zh",
  },
} as const satisfies Record<
  Locale,
  { htmlLang: string; label: string; path: string | undefined }
>;

function isLocale(value: string | undefined): value is Locale {
  return value !== undefined && (locales as readonly string[]).includes(value);
}

export function requireLocale(value: string | undefined): Locale {
  if (value === localeConfig.zh.htmlLang) return "zh";
  if (!isLocale(value)) {
    throw new Error(`Unsupported website locale: ${value ?? "undefined"}`);
  }
  return value;
}

export function localeFromPath(pathname: string): Locale {
  return pathname === "/zh" || pathname.startsWith("/zh/") ? "zh" : "en";
}

export function localePath(locale: Locale, pathname = "/"): string {
  const normalized = pathname.startsWith("/") ? pathname : `/${pathname}`;
  const rootPath =
    normalized === "/zh" || normalized === "/zh/"
      ? "/"
      : normalized.startsWith("/zh/")
        ? normalized.slice(3)
        : normalized;

  if (locale === defaultLocale) return rootPath;
  return rootPath === "/" ? "/zh/" : `/zh${rootPath}`;
}

export function homeHref(locale: Locale): string {
  return localePath(locale);
}

export function docsHref(locale: Locale, route = ""): string {
  const cleanRoute = route.replace(/^\/+|\/+$/g, "");
  const path = cleanRoute ? `/docs/${cleanRoute}/` : "/docs/";
  return localePath(locale, path);
}

export function alternateLocale(locale: Locale): Locale {
  return locale === "en" ? "zh" : "en";
}
