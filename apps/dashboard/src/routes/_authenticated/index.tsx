import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { createFileRoute, Link } from "@tanstack/react-router";
import {
  CloudflareProductIcon,
  type CloudflareProductName,
} from "../../components/cloudflare-product-icons";
import { PageHeader, Section } from "../../components/dashboard-page";

export const Route = createFileRoute("/_authenticated/")({
  component: AccountHomePage,
});

const products: readonly {
  name: string;
  description: string;
  href: string;
  icon: CloudflareProductName;
}[] = [
  {
    name: "Workers",
    description: "Deploy and manage serverless applications.",
    href: "/workers",
    icon: "Workers",
  },
  {
    name: "KV",
    description: "Read and write globally addressed key-value data.",
    href: "/kv",
    icon: "KV",
  },
  {
    name: "D1",
    description: "Build with serverless SQL databases.",
    href: "/d1",
    icon: "D1",
  },
  {
    name: "R2",
    description: "Store objects with an S3-compatible API.",
    href: "/r2",
    icon: "R2",
  },
  {
    name: "Durable Objects",
    description: "Inspect stateful Worker namespaces.",
    href: "/durable-objects",
    icon: "Durable Objects",
  },
  {
    name: "Queues",
    description: "Connect producers to reliable consumers.",
    href: "/queues",
    icon: "Queues",
  },
  {
    name: "Workflows",
    description: "Run durable multi-step applications.",
    href: "/workflows",
    icon: "Workflows",
  },
  {
    name: "Vectorize",
    description: "Store and query vector embeddings.",
    href: "/vectorize",
    icon: "Vectorize",
  },
  {
    name: "AI Search",
    description: "Build retrieval-backed AI applications.",
    href: "/ai-search",
    icon: "AI Search",
  },
];

function AccountHomePage() {
  return (
    <div className="grid gap-8">
      <PageHeader
        title="Account home"
        description="Manage compute, storage and AI resources on this open-compute installation."
      />
      <Section
        title="Build"
        description="Choose a product to create or manage resources."
      >
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {products.map((product) => {
            return (
              <LayerCard key={product.href} className="overflow-hidden p-0">
                <Link
                  to={product.href}
                  className="hover:bg-kumo-tint flex h-full min-h-28 gap-3 px-4 py-4"
                >
                  <span className="bg-kumo-info-tint text-kumo-brand flex size-9 shrink-0 items-center justify-center rounded-lg">
                    <CloudflareProductIcon product={product.icon} size={20} />
                  </span>
                  <span className="grid content-start gap-1">
                    <span className="font-medium">{product.name}</span>
                    <span className="text-kumo-subtle text-sm">
                      {product.description}
                    </span>
                  </span>
                </Link>
              </LayerCard>
            );
          })}
        </div>
      </Section>
    </div>
  );
}
