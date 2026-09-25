import { createFileRoute } from "@tanstack/react-router";
import { Notice, PageHeader } from "../../../components/dashboard-page";

export const Route = createFileRoute("/_authenticated/containers/")({
  component: ContainersPage,
});

function ContainersPage() {
  return (
    <div>
      <PageHeader
        title="Containers"
        description="Deploy and manage containerized workloads alongside your Workers."
      />
      <Notice>
        This product is in development and is not yet available in this
        installation.
      </Notice>
    </div>
  );
}
