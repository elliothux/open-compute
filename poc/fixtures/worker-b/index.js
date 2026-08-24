import { value } from "./dep.js";

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/body") {
      const text = await request.text();
      return Response.json({ deployment: "B", body: text, module: value });
    }
    return Response.json({
      deployment: "B",
      module: value,
      entrypoint: "default",
      identity: env.G0_IDENTITY ?? null,
    });
  },
};

export const extra = {
  async fetch(request, env) {
    return Response.json({
      deployment: "B",
      module: value,
      entrypoint: "extra",
      identity: env.G0_IDENTITY ?? null,
    });
  },
};
