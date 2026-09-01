import Link from "next/link";

export default function NavigationPage() {
  return (
    <main>
      <h1 data-testid="navigation-marker">client-navigation:ready</h1>
      <Link href="/">home</Link>
    </main>
  );
}
