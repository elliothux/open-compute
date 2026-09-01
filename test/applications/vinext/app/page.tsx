import Link from "next/link";
import { qualifyAction } from "./actions";
import { Counter } from "./components/counter";

export const dynamic = "force-dynamic";

export default function HomePage() {
  return (
    <main>
      <h1>open-compute P4 vinext qualification</h1>
      <p data-testid="server-marker">app-router:ssr</p>
      <Counter />
      <form action={qualifyAction}>
        <input name="marker" type="hidden" value="qualified" />
        <button data-testid="server-action" type="submit">run server action</button>
      </form>
      <nav>
        <Link href="/navigation" prefetch={false}>navigation</Link>{" | "}
        <Link href="/pages-qualification">pages router</Link>{" | "}
        <Link href="/stream">stream</Link>{" | "}
        <Link href="/static-qualification/alpha">static page</Link>{" | "}
        <Link href="/api/status?code=201">route handler</Link>
      </nav>
    </main>
  );
}
