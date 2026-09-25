import {
  CloudflareProductIcon,
  type CloudflareProductName,
} from "./cloudflare-product-icons";

const products = [
  "Workers",
  "Durable Objects",
  "KV",
  "D1",
  "R2",
  "Queues",
  "Workflows",
  "Cron Triggers",
  "Cache",
  "Images",
  "Vectorize",
  "AI Search",
  "Dynamic Workers",
  "Static Assets",
  "Service Bindings",
  "Artifacts",
  "Browser Run",
  "Containers",
  "Sandbox",
  "LogTail",
] as const satisfies readonly CloudflareProductName[];

function ProductTile({ product }: { product: CloudflareProductName }) {
  return (
    <span className="flex h-9 shrink-0 items-center gap-1.5 text-sm">
      <span className="text-kumo-subtle [&_svg]:block [&_svg]:size-4">
        <CloudflareProductIcon product={product} />
      </span>
      <span className="text-kumo-default whitespace-nowrap">{product}</span>
    </span>
  );
}

/** Auto-scrolling carousel of the Cloudflare products open-compute covers. */
export function CompatibilityMarquee() {
  return (
    <div className="login-marquee" aria-label="Cloudflare compatibility">
      <p className="text-kumo-subtle mb-3 text-xs font-medium">
        Cloudflare compatibility
      </p>
      <div className="overflow-hidden [mask-image:linear-gradient(to_right,transparent,black_24px,black_calc(100%_-_24px),transparent)]">
        <div className="login-marquee__track flex w-max gap-6">
          <div className="flex gap-6">
            {products.map((product) => (
              <ProductTile key={product} product={product} />
            ))}
          </div>
          <div aria-hidden="true" className="flex gap-6">
            {products.map((product) => (
              <ProductTile key={product} product={product} />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
