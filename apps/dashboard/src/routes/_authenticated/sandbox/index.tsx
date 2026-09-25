import { createFileRoute } from "@tanstack/react-router";
import { Notice, PageHeader } from "../../../components/dashboard-page";

export const Route = createFileRoute("/_authenticated/sandbox/")({
  component: SandboxPage,
});

function SandboxPage() {
  return (
    <div>
      <PageHeader
        title="Sandbox"
        description="Execute dynamic or untrusted code in isolated sandbox environments from your Workers."
      />
      <Notice>
        This product is in development and is not yet available in this
        installation.
      </Notice>
    </div>
  );
}
