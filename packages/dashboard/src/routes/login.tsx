import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { Button } from "@cloudflare/kumo/components/button";
import { BrandLogo } from "../components/BrandLogo";
import { Surface } from "@cloudflare/kumo/components/surface";
import { Input } from "@cloudflare/kumo/components/input";
import { APIError } from "cloudflare/error";
import { createManagementClient } from "../lib/cloudflare";
import { useAuth } from "../features/auth/AuthProvider";
import { mintSessionFromAdmin, writeAuthSession } from "../features/auth/authSession";

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
      const session = await mintSessionFromAdmin(trimmed);
      const nextClient = createManagementClient(session.session_token);
      const accounts = await nextClient.cloudflare.accounts.list();
      const account = accounts.result[0];
      if (account?.id === undefined) throw new Error("No accessible account was returned.");
      writeAuthSession(session.session_token, account.id);
      setToken(session.session_token);
      setAccountId(account.id);
      await navigate({ to: "/" });
    } catch (caught) {
      setToken(null);
      if (caught instanceof APIError) {
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
              Enter your admin token. open-compute exchanges it for a short-lived browser session
              stored only in this tab until you sign out or the session expires.
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
