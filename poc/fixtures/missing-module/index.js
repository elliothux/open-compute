import { missing } from "./does-not-exist.js";

export default {
  fetch() {
    return new Response(String(missing));
  },
};
