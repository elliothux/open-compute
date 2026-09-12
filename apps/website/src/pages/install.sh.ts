import installer from "../../../../scripts/install.sh?raw";

export const prerender = true;

export function GET(): Response {
  return new Response(installer, {
    headers: {
      "Content-Type": "text/x-shellscript; charset=utf-8",
      "Cache-Control": "public, max-age=300",
    },
  });
}
