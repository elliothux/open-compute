import type { QueryClient } from "@tanstack/react-query";
import {
  createRootRouteWithContext,
  Outlet,
  redirect,
} from "@tanstack/react-router";
import type { useAuth } from "../features/auth/auth-atoms";

export interface RouterContext {
  queryClient: QueryClient;
  auth: ReturnType<typeof useAuth>;
}

export const Route = createRootRouteWithContext<RouterContext>()({
  beforeLoad: ({ context, location }) => {
    const isLogin = location.pathname === "/login";
    if (!context.auth.token && !isLogin) {
      throw redirect({ to: "/login" });
    }
    if (context.auth.token && isLogin) {
      throw redirect({ to: "/" });
    }
  },
  component: () => <Outlet />,
});
