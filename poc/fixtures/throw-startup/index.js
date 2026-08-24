throw new Error("g0-startup-throw");

export default {
  fetch() {
    return new Response("should-not-run");
  },
};
