import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { IconFolders, IconSparkles, IconTag } from "@tabler/icons-react";
import { useState } from "react";
import { productIcon } from "./cloudflare-product-icons";
import { CodeBlock } from "./code-block";
import {
  resourceBindingLabels,
  type BindingKind,
} from "./worker-resource-binding-save";

const choices = [
  {
    kind: "kv_namespace",
    icon: productIcon("KV"),
    description: "Store and retrieve key-value data from your Worker.",
    detail:
      "Workers KV is a globally distributed data store for read-heavy workloads.",
    docs: "https://developers.cloudflare.com/kv/api/workers-kv/",
    example:
      "await env.KV.put('KEY', 'VALUE');\nconst value = await env.KV.get('KEY');",
  },
  {
    kind: "d1",
    icon: productIcon("D1"),
    description: "Store relational data in a serverless SQL database.",
    detail: "Query D1 from your Worker using prepared statements.",
    docs: "https://developers.cloudflare.com/d1/",
    example:
      "const result = await env.MY_DB.prepare(\n  'SELECT * FROM records LIMIT 100',\n).all();",
  },
  {
    kind: "durable_object_namespace",
    icon: productIcon("Durable Objects"),
    description: "Coordinate requests with strongly consistent state.",
    detail: "Bind a Durable Object class exported by this Worker.",
    docs: "https://developers.cloudflare.com/durable-objects/",
    example:
      "const id = env.MY_DURABLE_OBJECT.idFromName(\n  new URL(request.url).pathname,\n);\nconst stub = env.MY_DURABLE_OBJECT.get(id);",
  },
  {
    kind: "images",
    icon: productIcon("Images"),
    description: "Transform and encode images from your Worker.",
    detail:
      "Use the Images binding for supported raster transforms without a separate resource selector.",
    docs: "https://developers.cloudflare.com/images/transform-images/bindings/",
    example:
      "const image = request.body;\nconst response = await env.IMAGES.input(image)\n  .transform({ width: 400 })\n  .output({ format: 'image/avif' });\nreturn response.response();",
  },
  {
    kind: "ai",
    icon: IconSparkles,
    description: "Convert supported documents to Markdown from your Worker.",
    detail:
      "This installation supports the Workers AI binding for Markdown Conversion. General model inference is not available.",
    docs: "https://developers.cloudflare.com/workers-ai/features/markdown-conversion/usage/binding/",
    example:
      "const formats = await env.AI.toMarkdown().supported();\nreturn Response.json(formats);",
  },
  {
    kind: "ai_search",
    icon: productIcon("AI Search"),
    description: "Connect directly to one AI Search instance.",
    detail: "Search and chat with indexed content from your Worker.",
    docs: "https://developers.cloudflare.com/ai-search/api/search/workers-binding/",
    example:
      'const results = await env.MY_SEARCH.search({\n  query: "How does caching work?",\n});\nreturn Response.json(results);',
  },
  {
    kind: "ai_search_namespace",
    icon: IconFolders,
    description: "Access instances in an AI Search namespace.",
    detail: "Choose an instance at runtime, then search its indexed content.",
    docs: "https://developers.cloudflare.com/ai-search/api/search/workers-binding/",
    example:
      'const instance = env.AI_SEARCH.get("my-instance");\nconst results = await instance.search({\n  query: "How does caching work?",\n});\nreturn Response.json(results);',
  },
  {
    kind: "queue",
    icon: productIcon("Queues"),
    description: "Send messages reliably from your Worker.",
    detail: "Put work on a Queue for asynchronous processing.",
    docs: "https://developers.cloudflare.com/queues/configuration/javascript-apis/",
    example:
      "await env.MY_QUEUE.send({\n  url: request.url,\n  method: request.method,\n});",
  },
  {
    kind: "service",
    icon: productIcon("Service Bindings"),
    description: "Call another Worker in this account without a public URL.",
    detail: "Select the Worker that this binding will call.",
    docs: "https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/",
    example: "return env.MY_SERVICE.fetch(request);",
  },
  {
    kind: "r2_bucket",
    icon: productIcon("R2"),
    description: "Access objects in an R2 bucket from your Worker.",
    detail: "Read and write objects using the R2 Workers API.",
    docs: "https://developers.cloudflare.com/r2/api/workers/workers-api-reference/",
    example:
      "const object = await env.MY_BUCKET.get('key');\nawait env.MY_BUCKET.put('key', request.body);",
  },
  {
    kind: "version_metadata",
    icon: IconTag,
    description: "Expose metadata about the Worker, such as versionId.",
    detail:
      "Read the current immutable Version ID, tag and timestamp from your Worker.",
    docs: "https://developers.cloudflare.com/workers/runtime-apis/bindings/version-metadata/",
    example:
      "const { id: versionId, tag: versionTag, timestamp: versionTimestamp } = env.CF_VERSION_METADATA;\nreturn Response.json({ versionId, versionTag, versionTimestamp });",
  },
  {
    kind: "worker_loader",
    icon: productIcon("Dynamic Workers"),
    description: "Create and run Workers dynamically from your Worker.",
    detail: "Use a Worker Loader binding to load code at runtime.",
    docs: "https://developers.cloudflare.com/dynamic-workers/api-reference/",
    example:
      "const worker = env.LOADER.load({\n  compatibilityDate: '2026-09-01',\n  mainModule: 'index.js',\n  modules: { 'index.js': 'export default { fetch() { return new Response(\"OK\") } }' },\n});\nreturn worker.getEntrypoint().fetch(request);",
  },
  {
    kind: "vectorize",
    icon: productIcon("Vectorize"),
    description:
      "Store and query vector data in a globally distributed database.",
    detail:
      "Build AI-powered applications with a Vectorize index connected to your Worker.",
    docs: "https://developers.cloudflare.com/vectorize/reference/client-api/",
    example:
      "const queryVector = [32.4, 6.55, 11.2, 10.3, 87.9];\nconst matches = await env.MY_INDEX.query(queryVector);",
  },
] as const;

export function WorkerBindingGallery({
  open,
  onClose,
  onChoose,
}: {
  open: boolean;
  onClose: () => void;
  onChoose: (kind: BindingKind) => void;
}) {
  const [selected, setSelected] = useState<BindingKind>("kv_namespace");
  const choice = choices.find((item) => item.kind === selected) ?? choices[0];
  return (
    <Dialog.Root open={open} onOpenChange={(next) => !next && onClose()}>
      <Dialog className="flex max-h-dvh flex-col overflow-hidden p-0" size="xl">
        <div className="border-kumo-line flex flex-wrap items-baseline gap-2 border-b px-5 py-4">
          <Dialog.Title className="text-xl font-medium">
            Add binding
          </Dialog.Title>
          <Dialog.Description className="text-kumo-subtle">
            Connect an external resource to your Worker.
          </Dialog.Description>
        </div>
        <div className="grid min-h-0 flex-1 overflow-auto sm:grid-cols-4">
          <div
            role="listbox"
            aria-label="Supported bindings"
            className="border-kumo-line flex min-w-0 gap-1 overflow-auto p-3 sm:flex-col sm:border-r"
          >
            {choices.map((item) => {
              const Icon = item.icon;
              return (
                <button
                  key={item.kind}
                  type="button"
                  role="option"
                  aria-selected={item.kind === selected}
                  className={`hover:bg-kumo-tint flex shrink-0 items-center gap-2 rounded-lg px-3 py-2 text-left sm:w-full ${item.kind === selected ? "bg-kumo-recessed font-medium" : ""}`}
                  onClick={() => setSelected(item.kind)}
                >
                  <Icon size={16} className="shrink-0" />
                  {resourceBindingLabels[item.kind]}
                </button>
              );
            })}
          </div>
          <article className="min-w-0 space-y-5 overflow-auto px-5 py-4 sm:col-span-3">
            <div className="flex items-start justify-between gap-3">
              <div>
                <h3 className="text-xl font-semibold">
                  {resourceBindingLabels[choice.kind]}
                </h3>
                <p className="text-kumo-subtle mt-1">{choice.description}</p>
              </div>
              <a
                className="text-kumo-brand shrink-0 hover:underline"
                href={choice.docs}
                target="_blank"
                rel="noreferrer"
              >
                Documentation ↗
              </a>
            </div>
            <p>{choice.detail}</p>
            <CodeBlock
              className="ring-kumo-line min-w-0 overflow-auto rounded-lg p-4 text-xs ring"
              code={`export default {\n  async fetch(request, env) {\n    ${choice.example.replaceAll("\n", "\n    ")}\n    return new Response('OK');\n  },\n}`}
              language="javascript"
            />
          </article>
        </div>
        <div className="border-kumo-line flex justify-between border-t px-5 py-4">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => onChoose(selected)}>
            Add binding
          </Button>
        </div>
      </Dialog>
    </Dialog.Root>
  );
}
