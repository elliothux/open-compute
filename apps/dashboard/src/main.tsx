import { TooltipProvider } from "@cloudflare/kumo/components/tooltip";
import { LinkProvider, type LinkComponentProps } from "@cloudflare/kumo/utils";
import {
  QueryCache,
  QueryClient,
  QueryClientProvider,
  useQueryClient,
} from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { APIError } from "cloudflare/error";
import { Provider as JotaiProvider } from "jotai";
import { forwardRef, StrictMode, useMemo, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { AppRouter } from "./app-router";
import { useAuth } from "./features/auth/auth-atoms";
import { AuthBootstrap } from "./features/auth/auth-bootstrap";
import { ThemeSync } from "./features/theme/theme-sync";
import { ToastBridge } from "./features/toast/toast-bridge";
import "./app.css";

function redirectToLogin() {
  window.location.replace("/operator/login");
}

function AuthenticatedQueryProvider({ children }: { children: ReactNode }) {
  const { clearAuth } = useAuth();
  const queryClient = useMemo(() => {
    const client = new QueryClient({
      queryCache: new QueryCache({
        onError: (error) => {
          if (error instanceof APIError && error.status === 401) {
            clearAuth();
            client.clear();
            redirectToLogin();
          }
        },
      }),
      defaultOptions: {
        queries: {
          retry(failureCount, error) {
            if (error instanceof APIError) {
              if (error.status === 401 || error.status === 403) return false;
              if (error.status >= 400 && error.status < 500) return false;
            }
            return failureCount < 2;
          },
          staleTime: 15_000,
          refetchOnWindowFocus: false,
        },
        mutations: {
          retry: false,
          onError: (error) => {
            if (error instanceof APIError && error.status === 401) {
              clearAuth();
              client.clear();
              redirectToLogin();
            }
          },
        },
      },
    });
    return client;
  }, [clearAuth]);

  return (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

function AuthAwareRouter() {
  const queryClient = useQueryClient();
  return <AppRouter queryClient={queryClient} />;
}

const DashboardLink = forwardRef<HTMLAnchorElement, LinkComponentProps>(
  ({ href, onClick, target, ...props }, ref) => {
    const navigate = useNavigate();
    return (
      <a
        ref={ref}
        href={href}
        target={target}
        {...props}
        onClick={(event) => {
          onClick?.(event);
          if (
            !event.defaultPrevented &&
            event.button === 0 &&
            !event.metaKey &&
            !event.ctrlKey &&
            !event.shiftKey &&
            !event.altKey &&
            target !== "_blank" &&
            href?.startsWith("/")
          ) {
            event.preventDefault();
            void navigate({ to: href });
          }
        }}
      />
    );
  },
);
DashboardLink.displayName = "DashboardLink";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <JotaiProvider>
      <ThemeSync />
      <LinkProvider component={DashboardLink}>
        <TooltipProvider>
          <ToastBridge>
            <AuthBootstrap>
              <AuthenticatedQueryProvider>
                <AuthAwareRouter />
              </AuthenticatedQueryProvider>
            </AuthBootstrap>
          </ToastBridge>
        </TooltipProvider>
      </LinkProvider>
    </JotaiProvider>
  </StrictMode>,
);
