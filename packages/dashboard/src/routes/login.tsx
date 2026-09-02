import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { Button } from "@cloudflare/kumo/components/button";
import { BrandLogo } from "../components/BrandLogo";
import { Surface } from "@cloudflare/kumo/components/surface";
import { Input } from "@cloudflare/kumo/components/input";
import { createOperatorClient, OperatorApiError } from "@open-compute/operator-sdk";
import { useAuth } from "../features/auth/AuthProvider";

export const Route = createFileRoute("/login")({
  component: LoginPage,
});

function LoginPage() {
  const { setToken, setAccountId } = useAuth();
  const navigate = useNavigate();
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  async function onSubmit(event: React.FormEvent) {
    event.preventDefault();
    setError(null);
    setPending(true);
    try {
      const trimmed = value.trim();
      if (!trimmed) {
        setError("Enter an admin token to continue.");
        return;
      }
      const nextClient = createOperatorClient({
        baseUrl: new URL("/operator/api/v1/", window.location.origin),
        getAccessToken: () => trimmed,
      });
      const account = await nextClient.system.account();
      setToken(trimmed);
      setAccountId(account.accountId);
      await navigate({ to: "/" });
    } catch (caught) {
      setToken(null);
      if (caught instanceof OperatorApiError) {
        setError(caught.message);
      } else {
        setError("Unable to verify the admin token.");
      }
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="flex min-h-full items-center justify-center bg-kumo-base px-4 py-12">
      <Surface className="w-full max-w-md p-8">
        <div className="mb-6 space-y-4">
          <BrandLogo variant="wordmark" className="h-8 w-auto" />
          <div>
            <h1 className="text-xl font-semibold">Operator sign in</h1>
            <p className="text-sm text-kumo-subtle">
              Enter your admin token. Credentials stay in this tab&apos;s session storage until you sign out or close the tab.
            </p>
          </div>
        </div>
        <form className="space-y-4" onSubmit={onSubmit}>
          <Input
            id="token"
            label="Admin token"
            type="password"
            autoComplete="off"
            value={value}
            onChange={event => setValue(event.target.value)}
            placeholder="Bearer token value"
          />
          {error ? <div className="text-sm text-kumo-danger">{error}</div> : null}
          <Button type="submit" className="w-full" disabled={pending}>
            {pending ? "Verifying…" : "Continue"}
          </Button>
        </form>
      </Surface>
    </div>
  );
}