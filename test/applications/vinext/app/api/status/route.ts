const allowed = new Set([200, 201, 202, 400, 404]);

export function GET(request: Request): Response {
  const requested = Number(new URL(request.url).searchParams.get("code") ?? "200");
  const status = allowed.has(requested) ? requested : 400;
  return Response.json({ router: "app", status }, {
    status,
    headers: {
      "cache-control": "no-store",
      "set-cookie": "p4-app=qualified; HttpOnly; SameSite=Strict; Path=/",
      "x-p4-router": "app",
    },
  });
}
