import { createRouter } from "@tanstack/react-router";
import { routeTree } from "./routeTree.gen";
import type { RouterContext } from "./routes/__root";

export const router = createRouter({
  routeTree,
  basepath: "/operator",
  context: {
    queryClient: undefined!,
    auth: undefined!,
  } satisfies RouterContext,
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
