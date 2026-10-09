export interface StackRoute {
  stack: string;
  resource: string;
  segments: string[];
}

export function parseStackRoute(pathname: string): StackRoute | null {
  const parts = pathname.split("/").filter(Boolean);
  if (parts.length < 3 || parts[0] !== "stack") {
    return null;
  }
  return {
    stack: parts[1] ?? "",
    resource: parts[2] ?? "",
    segments: parts.slice(3),
  };
}

export function decodeKeySegment(segment: string): string {
  return decodeURIComponent(segment);
}
