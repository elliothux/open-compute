export default {
  async fetch() {
    try {
      const resp = await fetch("https://example.com/");
      return Response.json({
        outbound: "unexpected-success",
        status: resp.status,
      });
    } catch (err) {
      return Response.json({
        outbound: "denied",
        error: String(err && err.message ? err.message : err),
      });
    }
  },
};
