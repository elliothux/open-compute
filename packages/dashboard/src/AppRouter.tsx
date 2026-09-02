import { RouterProvider } from "@tanstack/react-router";
import { useAuth } from "./features/auth/AuthProvider";
import { router } from "./router";
import type { QueryClient } from "@tanstack/react-query";

export function AppRouter({ queryClient }: { queryClient: QueryClient }) {
  const auth = useAuth();
  return <RouterProvider router={router} context={{ queryClient, auth }} />;
}
