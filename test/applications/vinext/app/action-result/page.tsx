import { cookies } from "next/headers";
import Link from "next/link";

interface Props {
  readonly searchParams: Promise<{ readonly marker?: string }>;
}

export default async function ActionResultPage({ searchParams }: Props) {
  const query = await searchParams;
  const jar = await cookies();
  return (
    <main>
      <h1 data-testid="action-result">action:{query.marker ?? "missing"}</h1>
      <p data-testid="action-cookie">cookie:{jar.get("p4-action")?.value ?? "missing"}</p>
      <Link href="/">home</Link>
    </main>
  );
}
