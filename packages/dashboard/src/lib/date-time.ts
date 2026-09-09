import { format, formatDistance, isValid, parseISO, toDate } from "date-fns";
import { systemClock, type Clock } from "./clock";

function epochDate(epochMs: number): Date | null {
  if (!Number.isFinite(epochMs)) return null;
  const value = toDate(epochMs);
  return isValid(value) ? value : null;
}

export function formatTimestamp(epochMs: number | undefined | null): string {
  if (epochMs === undefined || epochMs === null) return "—";
  const value = epochDate(epochMs);
  return value === null ? "—" : format(value, "MMM d, yyyy HH:mm:ss");
}

export function formatRelative(
  epochMs: number | undefined | null,
  clock: Clock = systemClock,
): string {
  if (epochMs === undefined || epochMs === null) return "—";
  const value = epochDate(epochMs);
  const now = epochDate(clock.now());
  return value === null || now === null
    ? "—"
    : formatDistance(value, now, { addSuffix: true });
}

export function normalizeRfc3339(value: string | number): string | null {
  const parsed = typeof value === "number" ? epochDate(value) : parseISO(value);
  return parsed === null || !isValid(parsed) ? null : parsed.toISOString();
}

export function currentEpochMs(clock: Clock = systemClock): number {
  return clock.now();
}
