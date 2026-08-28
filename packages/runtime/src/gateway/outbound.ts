export default {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (url.protocol !== "http:" && url.protocol !== "https:") {
      throw new TypeError("OUTBOUND_DENIED");
    }
    return fetch(new Request(request, { redirect: "follow" }));
  },
};
