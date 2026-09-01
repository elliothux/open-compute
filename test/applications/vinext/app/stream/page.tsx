import { Suspense } from "react";

export const dynamic = "force-dynamic";

async function ResolvedMarker() {
  await new Promise(resolve => setTimeout(resolve, 50));
  return <p data-testid="stream-resolved">stream:resolved</p>;
}

export default function StreamPage() {
  return (
    <main>
      <h1>streaming qualification</h1>
      <Suspense fallback={<p data-testid="stream-fallback">stream:fallback</p>}>
        <ResolvedMarker />
      </Suspense>
    </main>
  );
}
