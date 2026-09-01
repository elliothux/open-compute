import type { GetStaticPaths, GetStaticProps } from "next";

interface Props {
  readonly slug: string;
}

export const getStaticPaths: GetStaticPaths = () => ({
  paths: [{ params: { slug: "alpha" } }, { params: { slug: "beta" } }],
  fallback: false,
});

export const getStaticProps: GetStaticProps<Props> = context => ({
  props: { slug: String(context.params?.slug ?? "missing") },
});

export default function StaticQualification({ slug }: Props) {
  return <main><h1 data-testid="gsp-marker">gsp:{slug}</h1></main>;
}
