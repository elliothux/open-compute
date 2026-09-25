import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { Select } from "@cloudflare/kumo/components/select";
import { IconArrowLeft, IconArrowRight, IconUpload } from "@tabler/icons-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { CloudflareProductIcon } from "../../../components/cloudflare-product-icons";
import { CreateStepper } from "../../../components/create-stepper";
import { useAuth } from "../../../features/auth/auth-atoms";

export const Route = createFileRoute("/_authenticated/ai-search/new")({
  component: CreateAISearchPage,
});

type Source = "builtin" | "r2";

function CreateAISearchPage() {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [name, setName] = useState(
    () => `ai-search-${crypto.randomUUID().slice(0, 8)}`,
  );
  const [namespace, setNamespace] = useState("");
  const [source, setSource] = useState<Source>("builtin");
  const [bucket, setBucket] = useState("");
  const [bucketSearch, setBucketSearch] = useState("");
  const [prefix, setPrefix] = useState("");
  const [chunkSize, setChunkSize] = useState(512);
  const [chunkOverlap, setChunkOverlap] = useState(10);
  const [maxResults, setMaxResults] = useState(10);
  const [scoreThreshold, setScoreThreshold] = useState(0.4);
  const [step, setStep] = useState(0);
  const steps =
    source === "r2"
      ? ["Name and source", "Configure source", "Review settings", "Create"]
      : ["Name and source", "Review settings", "Create"];
  const nameValid =
    name.length <= 64 && /^[a-z0-9_]+(?:-[a-z0-9_]+)*$/.test(name);
  const settingsValid =
    Number.isInteger(chunkSize) &&
    chunkSize > 0 &&
    Number.isInteger(chunkOverlap) &&
    chunkOverlap >= 0 &&
    chunkOverlap <= 30 &&
    Number.isInteger(maxResults) &&
    maxResults >= 1 &&
    maxResults <= 50 &&
    Number.isFinite(scoreThreshold) &&
    scoreThreshold >= 0 &&
    scoreThreshold <= 1;

  const namespaces = useQuery({
    queryKey: ["ai-search", selectedInstanceId, "namespaces"],
    queryFn: ({ signal }) =>
      client!.aiSearch.namespaces.list(
        { account_id: selectedInstanceId!, per_page: 100 },
        { signal },
      ),
    enabled: client !== null && selectedInstanceId !== null,
  });
  const buckets = useQuery({
    queryKey: ["cloudflare-v4", "r2", selectedInstanceId, "buckets"],
    queryFn: ({ signal }) =>
      client!.r2.buckets.list({ account_id: selectedInstanceId! }, { signal }),
    enabled: client !== null && selectedInstanceId !== null && source === "r2",
  });
  const selectedNamespace = namespace || namespaces.data?.result[0]?.name || "";
  const visibleBuckets = (buckets.data?.buckets ?? []).filter((item) =>
    item.name?.toLowerCase().includes(bucketSearch.trim().toLowerCase()),
  );
  const create = useMutation({
    mutationFn: async () => {
      if (
        !client ||
        !selectedInstanceId ||
        !selectedNamespace ||
        !nameValid ||
        !settingsValid ||
        (source === "r2" && !bucket)
      ) {
        throw new Error("Complete the required instance settings.");
      }
      await client.aiSearch.namespaces.instances.create(selectedNamespace, {
        account_id: selectedInstanceId,
        id: name,
        ...(source === "r2"
          ? {
              type: "r2" as const,
              source: bucket,
              source_params: { prefix },
              sync_interval: 21600 as const,
            }
          : {}),
        chunk_size: chunkSize,
        chunk_overlap: chunkOverlap,
        max_num_results: maxResults,
        score_threshold: scoreThreshold,
      });
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: ["ai-search", selectedInstanceId],
      });
      await navigate({
        to: "/ai-search/$namespaceName/$instanceId",
        params: { namespaceName: selectedNamespace, instanceId: name },
        search: { tab: "overview" },
      });
    },
  });

  const next = () => {
    create.reset();
    setStep((value) => value + 1);
  };
  const back = () => {
    create.reset();
    setStep((value) => value - 1);
  };
  const last = steps.length - 1;

  return (
    <CreateStepper
      title="Create instance"
      steps={steps}
      current={step}
      onClose={() => void navigate({ to: "/ai-search" })}
    >
      <div className="bg-kumo-base overflow-hidden rounded-xl">
        {step === 0 ? (
          <div className="grid gap-5 px-6 py-5">
            <div>
              <h2 className="text-base font-semibold">Name your instance</h2>
              <p className="text-kumo-subtle mt-1">
                Name your AI Search instance and optionally connect a data
                source.
              </p>
            </div>
            <div>
              <Input
                label="Instance name"
                value={name}
                onChange={(event) => setName(event.target.value)}
                autoFocus
              />
              <p className="text-kumo-subtle mt-1 text-xs">
                Up to 64 lowercase letters, numbers and underscores, with
                hyphens between groups.
              </p>
              {name && !nameValid ? (
                <p className="text-kumo-danger mt-1 text-xs">
                  Enter a valid instance name.
                </p>
              ) : null}
            </div>
            <Select
              label="Namespace"
              value={selectedNamespace}
              items={(namespaces.data?.result ?? []).map((item) => ({
                label: item.name,
                value: item.name,
              }))}
              onValueChange={(value) => setNamespace(value ?? "")}
            />
            {namespaces.error ? (
              <p role="alert" className="text-kumo-danger">
                Unable to load namespaces.
              </p>
            ) : null}
            {!namespaces.isLoading && !selectedNamespace ? (
              <p className="text-kumo-warning">
                Create a namespace on the AI Search page before creating an
                instance.
              </p>
            ) : null}
            <fieldset className="grid gap-2">
              <legend className="mb-2 font-medium">Data source</legend>
              <label className="border-kumo-line flex min-h-20 items-center gap-3 rounded-lg border px-4">
                <input
                  type="radio"
                  name="source"
                  value="builtin"
                  checked={source === "builtin"}
                  onChange={() => setSource("builtin")}
                />
                <IconUpload size={18} />
                <span>
                  <span className="block font-medium">Built-in storage</span>
                  <span className="text-kumo-subtle">
                    Upload and manage files directly.
                  </span>
                </span>
              </label>
              <p className="text-kumo-subtle text-xs">
                Optionally connect an additional data source.
              </p>
              <label className="border-kumo-line flex min-h-20 items-center gap-3 rounded-lg border px-4">
                <input
                  type="radio"
                  name="source"
                  value="r2"
                  checked={source === "r2"}
                  onChange={() => setSource("r2")}
                />
                <CloudflareProductIcon product="D1" size={18} />
                <span>
                  <span className="block font-medium">R2 bucket</span>
                  <span className="text-kumo-subtle">
                    Index files in an existing bucket.
                  </span>
                </span>
              </label>
            </fieldset>
          </div>
        ) : source === "r2" && step === 1 ? (
          <div className="grid gap-5 px-6 py-5">
            <div>
              <h2 className="text-base font-semibold">Choose an R2 bucket</h2>
              <p className="text-kumo-subtle mt-1">
                Select the bucket containing your data.
              </p>
            </div>
            <Input
              label="Search buckets"
              placeholder="Search buckets"
              value={bucketSearch}
              onChange={(event) => setBucketSearch(event.target.value)}
            />
            {buckets.error ? (
              <p role="alert" className="text-kumo-danger">
                Unable to load R2 buckets.
              </p>
            ) : null}
            <div className="grid max-h-64 gap-2 overflow-y-auto">
              {visibleBuckets.map((item) =>
                item.name ? (
                  <label
                    key={item.name}
                    className="border-kumo-line flex items-center gap-3 rounded-lg border px-4 py-3"
                  >
                    <input
                      type="radio"
                      name="bucket"
                      value={item.name}
                      checked={bucket === item.name}
                      onChange={() => setBucket(item.name!)}
                    />
                    <CloudflareProductIcon product="D1" size={17} />
                    <span>{item.name}</span>
                  </label>
                ) : null,
              )}
            </div>
            {!buckets.isLoading && visibleBuckets.length === 0 ? (
              <p className="text-kumo-subtle">
                No matching buckets. Create one in R2 first.
              </p>
            ) : null}
            <Input
              label="Object key prefix (optional)"
              placeholder="docs/"
              value={prefix}
              onChange={(event) => setPrefix(event.target.value)}
            />
            <p className="text-kumo-subtle text-xs">
              OCD uses its installation-managed token for this same-account R2
              source. No browser credential is created.
            </p>
          </div>
        ) : step === last - 1 ? (
          <div className="grid gap-5 px-6 py-5">
            <div>
              <h2 className="text-base font-semibold">Review settings</h2>
              <p className="text-kumo-subtle mt-1">
                These values use OCD's supported indexing and retrieval
                controls.
              </p>
            </div>
            <details className="border-kumo-line rounded-lg border p-4">
              <summary className="cursor-pointer font-medium">
                Indexing · {chunkSize} tokens · {chunkOverlap}% overlap
              </summary>
              <div className="mt-4 grid grid-cols-2 gap-3">
                <Input
                  label="Chunk size"
                  type="number"
                  min={1}
                  value={chunkSize}
                  onChange={(event) => setChunkSize(Number(event.target.value))}
                />
                <Input
                  label="Overlap percent"
                  type="number"
                  min={0}
                  max={30}
                  value={chunkOverlap}
                  onChange={(event) =>
                    setChunkOverlap(Number(event.target.value))
                  }
                />
              </div>
            </details>
            <details className="border-kumo-line rounded-lg border p-4">
              <summary className="cursor-pointer font-medium">
                Retrieval · {maxResults} results · score {scoreThreshold}
              </summary>
              <div className="mt-4 grid grid-cols-2 gap-3">
                <Input
                  label="Maximum results"
                  type="number"
                  min={1}
                  max={50}
                  value={maxResults}
                  onChange={(event) =>
                    setMaxResults(Number(event.target.value))
                  }
                />
                <Input
                  label="Score threshold"
                  type="number"
                  min={0}
                  max={1}
                  step={0.05}
                  value={scoreThreshold}
                  onChange={(event) =>
                    setScoreThreshold(Number(event.target.value))
                  }
                />
              </div>
            </details>
            {!settingsValid ? (
              <p className="text-kumo-danger">
                Correct the indexing or retrieval values to continue.
              </p>
            ) : null}
          </div>
        ) : (
          <div className="grid gap-5 px-6 py-5">
            <div>
              <h2 className="text-base font-semibold">Create instance</h2>
              <p className="text-kumo-subtle mt-1">
                Review your choices and create the AI Search instance.
              </p>
            </div>
            <dl className="grid gap-2">
              {[
                ["Instance name", name],
                ["Namespace", selectedNamespace],
                [
                  "Data source",
                  source === "r2" ? `R2 · ${bucket}` : "Built-in storage",
                ],
              ].map(([label, value]) => (
                <div
                  key={label}
                  className="border-kumo-line flex justify-between gap-3 rounded-lg border px-4 py-3"
                >
                  <dt className="text-kumo-subtle">{label}</dt>
                  <dd className="font-medium">{value}</dd>
                </div>
              ))}
            </dl>
            {create.error ? (
              <p role="alert" className="text-kumo-danger">
                {create.error instanceof Error
                  ? create.error.message
                  : "Unable to create instance."}
              </p>
            ) : null}
          </div>
        )}
        <div className="border-kumo-line bg-kumo-tint flex justify-between border-t px-5 py-3">
          {step > 0 ? (
            <Button variant="ghost" onClick={back} disabled={create.isPending}>
              <IconArrowLeft size={15} /> Back
            </Button>
          ) : (
            <span />
          )}
          {step < last ? (
            <Button
              variant="primary"
              onClick={next}
              disabled={
                step === 0
                  ? !nameValid || !selectedNamespace
                  : source === "r2" && step === 1
                    ? !bucket
                    : !settingsValid
              }
            >
              Next <IconArrowRight size={15} />
            </Button>
          ) : (
            <Button
              variant="primary"
              onClick={() => create.mutate()}
              disabled={create.isPending}
            >
              {create.isPending ? "Creating…" : "Create"}
            </Button>
          )}
        </div>
      </div>
    </CreateStepper>
  );
}
