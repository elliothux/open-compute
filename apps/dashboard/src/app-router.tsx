import type { QueryClient } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { useAuth } from "./features/auth/auth-atoms";
import { router } from "./router";

export function AppRouter({ queryClient }: { queryClient: QueryClient }) {
  const auth = useAuth();
  return <RouterProvider router={router} context={{ queryClient, auth }} />;
}
