import { forwardRef, StrictMode, useMemo, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { useNavigate } from "@tanstack/react-router";
import { TooltipProvider } from "@cloudflare/kumo/components/tooltip";
import { LinkProvider, type LinkComponentProps } from "@cloudflare/kumo/utils";
import {
  QueryCache,
  QueryClient,
  QueryClientProvider,
  useQueryClient,
} from "@tanstack/react-query";
import { APIError } from "cloudflare/error";
import { AuthProvider, useAuth } from "./features/auth/AuthProvider";
import { ThemeProvider } from "./features/theme/ThemeProvider";
import { ToastProvider } from "./features/toast/ToastProvider";
import { AppRouter } from "./AppRouter";
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

  return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
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
        onClick={event => {
          onClick?.(event);
          if (
            !event.defaultPrevented
            && event.button === 0
            && !event.metaKey
            && !event.ctrlKey
            && !event.shiftKey
            && !event.altKey
            && target !== "_blank"
            && href?.startsWith("/")
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
    <ThemeProvider>
      <LinkProvider component={DashboardLink}>
        <TooltipProvider>
          <ToastProvider>
            <AuthProvider>
              <AuthenticatedQueryProvider>
                <AuthAwareRouter />
              </AuthenticatedQueryProvider>
            </AuthProvider>
          </ToastProvider>
        </TooltipProvider>
      </LinkProvider>
    </ThemeProvider>
  </StrictMode>,
);
