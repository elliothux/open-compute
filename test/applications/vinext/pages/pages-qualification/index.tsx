import type { GetServerSidePropsResult } from "next";
import Link from "next/link";

interface Props {
  readonly marker: string;
}

export function getServerSideProps(): GetServerSidePropsResult<Props> {
  return { props: { marker: "pages-router:gssp" } };
}

export default function PagesQualification({ marker }: Props) {
  return (
    <main>
      <h1 data-testid="pages-marker">{marker}</h1>
      <Link href="/">app router</Link>
    </main>
  );
}
