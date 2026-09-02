import { format, formatDistanceToNow } from "date-fns";

export function formatTimestamp(epochMs: number | undefined | null): string {
  if (!epochMs) return "—";
  return format(new Date(epochMs), "MMM d, yyyy HH:mm:ss");
}

export function formatRelative(epochMs: number | undefined | null): string {
  if (!epochMs) return "—";
  return formatDistanceToNow(new Date(epochMs), { addSuffix: true });
}

export function formatBytes(bytes: number | undefined | null): string {
  if (bytes === undefined || bytes === null) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}
