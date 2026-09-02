const DOCS_ORIGIN = "https://open-compute.dev";

export const docsLinks = {
  home: `${DOCS_ORIGIN}/`,
  overview: `${DOCS_ORIGIN}/health`,
  workers: `${DOCS_ORIGIN}/deploy`,
  storage: `${DOCS_ORIGIN}/capabilities`,
  platform: `${DOCS_ORIGIN}/incidents/scheduler`,
} as const;
