export function processEnv(
  extra: Readonly<Record<string, string>>,
): Record<string, string> {
  const env: Record<string, string> = {
    ...extra,
    CI: "true",
    WRANGLER_HIDE_BANNER: "true",
    WRANGLER_SEND_METRICS: "false",
  };
  for (const name of ["PATH", "HOME", "TMPDIR", "TMP", "TEMP"]) {
    const value = process.env[name];
    if (value !== undefined) env[name] = value;
  }
  return env;
}
