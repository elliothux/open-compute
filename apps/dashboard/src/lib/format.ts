const BYTE_UNITS = ["B", "kB", "MB", "GB", "TB"] as const;
const DATE_FORMAT = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });
const DATE_TIME_FORMAT = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

function asDate(value: string | number | Date | null | undefined) {
  if (value === null || value === undefined || value === "") return null;
  const date = value instanceof Date ? value : new Date(value);
  return Number.isFinite(date.getTime()) ? date : null;
}

export function formatBytes(value: number | undefined): string {
  if (value === undefined || !Number.isFinite(value)) return "—";
  const bytes = Math.max(0, value);
  const unit = Math.min(
    Math.floor(Math.log(Math.max(bytes, 1)) / Math.log(1024)),
    BYTE_UNITS.length - 1,
  );
  const amount = bytes / 1024 ** unit;
  return `${unit === 0 ? amount.toFixed(0) : amount.toFixed(1)} ${BYTE_UNITS[unit]}`;
}

export function formatDate(
  value: string | number | Date | null | undefined,
): string {
  const date = asDate(value);
  return date ? DATE_FORMAT.format(date) : "—";
}

export function formatDateTime(
  value: string | number | Date | null | undefined,
): string {
  const date = asDate(value);
  return date ? DATE_TIME_FORMAT.format(date) : "—";
}
