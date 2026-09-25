import { createFileRoute } from "@tanstack/react-router";
import { Notice, PageHeader } from "../../../components/dashboard-page";

export const Route = createFileRoute("/_authenticated/browser-run/")({
  component: BrowserRunPage,
});

function BrowserRunPage() {
  return (
    <div>
      <PageHeader
        title="Browser Run"
        description="Run headless browser sessions and capture page snapshots from your Workers."
      />
      <Notice>
        This product is in development and is not yet available in this
        installation.
      </Notice>
    </div>
  );
}
