export default {
  fetch(request: Request): Response {
    const url = new URL(request.url);
    return Response.json({
      kind: "oc-p4-companion",
      path: url.pathname,
    });
  },
};
