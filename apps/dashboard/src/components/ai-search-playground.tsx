import { Button, LinkButton } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Popover } from "@cloudflare/kumo/components/popover";
import { Select } from "@cloudflare/kumo/components/select";
import { Switch } from "@cloudflare/kumo/components/switch";
import {
  IconAdjustments,
  IconCaretDown,
  IconCaretLeft,
  IconCaretRight,
  IconCopy,
  IconPlus,
  IconSearch,
  IconSend,
  IconTrash,
} from "@tabler/icons-react";
import { useMutation } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import type {
  InstanceChatCompletionsResponse,
  InstanceSearchResponse,
} from "cloudflare/resources/aisearch/namespaces/instances/instances";
import { useEffect, useRef, useState } from "react";
import { useAuth } from "../features/auth/auth-atoms";
import { useMutationFeedback } from "../features/toast/use-mutation-feedback";
import { EmptyState } from "./dashboard-page";

type MetadataField = {
  field_name: string;
  data_type: "text" | "number" | "boolean" | "datetime";
};
type FilterRow = { field: string; operator: string; value: string };
type Chunk =
  InstanceSearchResponse.Chunk | InstanceChatCompletionsResponse.Chunk;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseStreamChunks(value: unknown): Chunk[] {
  if (!Array.isArray(value)) throw new Error("Invalid chat sources response.");
  return value.map((entry: unknown) => {
    if (
      !isRecord(entry) ||
      typeof entry.id !== "string" ||
      typeof entry.type !== "string" ||
      typeof entry.text !== "string" ||
      typeof entry.score !== "number" ||
      !Number.isFinite(entry.score)
    ) {
      throw new Error("Invalid chat source.");
    }
    let item: Chunk["item"];
    if (entry.item !== undefined && entry.item !== null) {
      if (!isRecord(entry.item) || typeof entry.item.key !== "string") {
        throw new Error("Invalid chat source item.");
      }
      item = {
        key: entry.item.key,
        ...(typeof entry.item.timestamp === "number"
          ? { timestamp: entry.item.timestamp }
          : {}),
      };
    }
    let scoring_details: Chunk["scoring_details"];
    if (isRecord(entry.scoring_details)) {
      scoring_details = {
        ...(typeof entry.scoring_details.vector_score === "number"
          ? { vector_score: entry.scoring_details.vector_score }
          : {}),
        ...(typeof entry.scoring_details.keyword_score === "number"
          ? { keyword_score: entry.scoring_details.keyword_score }
          : {}),
        ...(typeof entry.scoring_details.reranking_score === "number"
          ? { reranking_score: entry.scoring_details.reranking_score }
          : {}),
      };
    }
    return {
      id: entry.id,
      type: entry.type,
      text: entry.text,
      score: entry.score,
      ...(item ? { item } : {}),
      ...(scoring_details ? { scoring_details } : {}),
    };
  });
}

async function readChatStream(
  response: Response,
  onChunks: (chunks: Chunk[]) => void,
  onDelta: (delta: string) => void,
) {
  if (!response.body) throw new Error("Chat response has no stream.");
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let done = false;
  try {
    while (true) {
      const next = await reader.read();
      buffer += decoder.decode(next.value, { stream: !next.done });
      buffer = buffer.replaceAll("\r\n", "\n");
      let end = buffer.indexOf("\n\n");
      while (end !== -1) {
        const frame = buffer.slice(0, end);
        buffer = buffer.slice(end + 2);
        const lines = frame.split("\n");
        const kind = lines.find((line) => line.startsWith("event: "))?.slice(7);
        const data = lines
          .filter((line) => line.startsWith("data: "))
          .map((line) => line.slice(6))
          .join("\n");
        if (data === "[DONE]") {
          done = true;
          break;
        }
        if (data) {
          const value: unknown = JSON.parse(data);
          if (kind === "chunks") {
            onChunks(parseStreamChunks(value));
          } else if (isRecord(value)) {
            const choice = Array.isArray(value.choices)
              ? value.choices[0]
              : undefined;
            if (
              isRecord(choice) &&
              isRecord(choice.delta) &&
              typeof choice.delta.content === "string"
            ) {
              onDelta(choice.delta.content);
            }
          } else {
            throw new Error("Invalid chat response event.");
          }
        }
        end = buffer.indexOf("\n\n");
      }
      if (done || next.done) break;
    }
  } finally {
    await reader.cancel();
  }
  if (!done) throw new Error("Chat stream ended before completion.");
}

function filterValue(row: FilterRow, field: MetadataField): unknown {
  switch (field.data_type) {
    case "number":
      return Number(row.value);
    case "boolean":
      return row.value === "true";
    case "datetime":
      return Date.parse(row.value);
    default:
      return row.value.trim();
  }
}

function makeFilters(rows: FilterRow[], fields: MetadataField[]) {
  const filters: Record<string, unknown> = {};
  if (rows.length > 10) return null;
  for (const row of rows) {
    const field = fields.find((entry) => entry.field_name === row.field);
    if (!field || row.value.trim() === "" || row.field in filters) return null;
    const value = filterValue(row, field);
    if (typeof value === "number" && !Number.isFinite(value)) return null;
    filters[row.field] =
      row.operator === "$eq" ? value : { [row.operator]: value };
  }
  return new TextEncoder().encode(JSON.stringify(filters)).length < 2_048
    ? filters
    : null;
}

function ChunkCard({
  chunk,
  namespaceName,
  instanceId,
  onCopy,
}: {
  chunk: Chunk;
  namespaceName: string;
  instanceId: string;
  onCopy: (text: string) => void;
}) {
  const key = chunk.item?.key;
  const details = chunk.scoring_details;
  const scores = [
    ["Vector score", details?.vector_score],
    ["Keyword score", details?.keyword_score],
    ["Reranking score", details?.reranking_score],
  ] as const;
  return (
    <article className="bg-kumo-base ring-kumo-line min-w-0 rounded-lg px-6 py-5 ring">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="text-kumo-subtle text-xs">ID: {chunk.id}</div>
          {key ? (
            <Link
              className="text-kumo-link mt-2 block text-sm break-all hover:underline"
              to="/ai-search/$namespaceName/$instanceId"
              params={{ namespaceName, instanceId }}
              search={{ tab: "items" }}
            >
              {key}
            </Link>
          ) : null}
          {chunk.item?.timestamp ? (
            <div className="text-kumo-subtle mt-1 text-xs">
              {new Date(
                chunk.item.timestamp < 1_000_000_000_000
                  ? chunk.item.timestamp * 1_000
                  : chunk.item.timestamp,
              ).toLocaleString()}
            </div>
          ) : null}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <Popover>
            <Popover.Trigger render={<Button variant="ghost" />}>
              Score {chunk.score.toFixed(3)}
            </Popover.Trigger>
            <Popover.Content className="w-56 p-3">
              <Popover.Title>Score details</Popover.Title>
              <dl className="mt-2 grid gap-2 text-sm">
                {scores.filter(([, score]) => score !== undefined).length ? (
                  scores.map(([label, score]) =>
                    score === undefined ? null : (
                      <div key={label} className="flex justify-between gap-3">
                        <dt className="text-kumo-subtle">{label}</dt>
                        <dd>{score.toFixed(3)}</dd>
                      </div>
                    ),
                  )
                ) : (
                  <div className="text-kumo-subtle">
                    No score breakdown available.
                  </div>
                )}
              </dl>
            </Popover.Content>
          </Popover>
          <Button
            variant="ghost"
            shape="square"
            aria-label="Copy result text"
            onClick={() => onCopy(chunk.text)}
          >
            <IconCopy size={16} />
          </Button>
        </div>
      </div>
      <div className="bg-kumo-recessed mt-4 rounded-lg px-3 py-3 text-sm break-words whitespace-pre-wrap">
        {chunk.text}
      </div>
    </article>
  );
}

export function AISearchPlayground({
  namespaceName,
  instanceId,
  metadataFields,
  generationModel,
}: {
  namespaceName: string;
  instanceId: string;
  metadataFields: MetadataField[];
  generationModel?: string | null | undefined;
}) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const feedback = useMutationFeedback();
  const [tab, setTab] = useState<"search" | "chat">("search");
  const [query, setQuery] = useState("");
  const [railOpen, setRailOpen] = useState(true);
  const [rewrite, setRewrite] = useState(false);
  const [reranking, setReranking] = useState(false);
  const [threshold, setThreshold] = useState(0.4);
  const [maximum, setMaximum] = useState(10);
  const [contextExpansion, setContextExpansion] = useState(0);
  const [filters, setFilters] = useState<FilterRow[]>([]);
  const [filterDraft, setFilterDraft] = useState<FilterRow[]>([]);
  const [filterOpen, setFilterOpen] = useState(false);
  const [filterError, setFilterError] = useState("");
  const [submittedSearch, setSubmittedSearch] = useState("");
  const [submittedChat, setSubmittedChat] = useState("");
  const [sourcesOpen, setSourcesOpen] = useState(false);
  const [chatText, setChatText] = useState("");
  const [chatChunks, setChatChunks] = useState<Chunk[]>([]);
  const chatAbort = useRef<AbortController | null>(null);
  useEffect(() => () => chatAbort.current?.abort(), []);
  const appliedFilters = makeFilters(filters, metadataFields);
  const options = {
    query_rewrite: { enabled: rewrite },
    reranking: { enabled: reranking },
    retrieval: {
      match_threshold: threshold,
      max_num_results: maximum,
      ...(tab === "chat" ? { context_expansion: contextExpansion } : {}),
      ...(appliedFilters && Object.keys(appliedFilters).length
        ? { filters: appliedFilters }
        : {}),
    },
  };
  const search = useMutation({
    mutationFn: (message: string) =>
      client!.aiSearch.namespaces.instances.search(instanceId, {
        account_id: selectedInstanceId!,
        name: namespaceName,
        messages: [{ role: "user", content: message }],
        ai_search_options: options,
      }),
    onError: (error) => feedback.failure(error, "Search failed."),
  });
  const chat = useMutation({
    mutationFn: async (message: string) => {
      const controller = new AbortController();
      chatAbort.current = controller;
      try {
        const response = await client!.aiSearch.namespaces.instances
          .chatCompletions(
            instanceId,
            {
              account_id: selectedInstanceId!,
              name: namespaceName,
              messages: [{ role: "user", content: message }],
              stream: true,
              ai_search_options: options,
            },
            { signal: controller.signal },
          )
          .asResponse();
        await readChatStream(response, setChatChunks, (delta) =>
          setChatText((current) => current + delta),
        );
      } catch (error) {
        if (!controller.signal.aborted) throw error;
      } finally {
        if (chatAbort.current === controller) chatAbort.current = null;
      }
    },
    onError: (error) => feedback.failure(error, "Chat failed."),
  });
  const pending = tab === "search" ? search.isPending : chat.isPending;
  const activeError = tab === "search" ? search.error : chat.error;
  const submit = () => {
    const message = query.trim();
    if (
      !message ||
      pending ||
      maximum === 0 ||
      !client ||
      !selectedInstanceId ||
      (tab === "chat" && !generationModel)
    )
      return;
    setQuery("");
    if (tab === "search") {
      setSubmittedSearch(message);
      search.reset();
      search.mutate(message);
    } else {
      setSubmittedChat(message);
      setSourcesOpen(false);
      setChatText("");
      setChatChunks([]);
      chatAbort.current?.abort();
      chat.reset();
      chat.mutate(message);
    }
  };
  const copyText = (value: string) => {
    void navigator.clipboard.writeText(value).then(
      () => feedback.success("Copied to clipboard."),
      (error: unknown) => feedback.failure(error, "Could not copy text."),
    );
  };
  const beginFilters = () => {
    setFilterDraft(filters.map((row) => ({ ...row })));
    setFilterError("");
    setFilterOpen(true);
  };

  return (
    <div className="bg-kumo-base -mx-3 flex min-h-[calc(100vh-164px)] min-w-0 flex-col overflow-hidden sm:-mx-6 lg:flex-row">
      <div className="bg-kumo-recessed min-w-0 flex-1">
        <div className="flex items-center justify-between px-7">
          <div
            className="flex gap-6"
            role="tablist"
            aria-label="Playground mode"
          >
            {(["search", "chat"] as const).map((mode) => (
              <button
                key={mode}
                type="button"
                role="tab"
                aria-selected={tab === mode}
                className={`border-b-2 py-3 text-sm ${tab === mode ? "border-kumo-link text-kumo-link font-medium" : "text-kumo-subtle hover:text-kumo-default border-transparent"}`}
                onClick={() => {
                  if (mode === tab) return;
                  chatAbort.current?.abort();
                  setQuery("");
                  setTab(mode);
                  if (mode === "search") {
                    setSubmittedSearch("");
                    search.reset();
                  } else {
                    setSubmittedChat("");
                    setChatText("");
                    setChatChunks([]);
                    chat.reset();
                  }
                }}
              >
                {mode === "search" ? "Search" : "Chat"}
              </button>
            ))}
          </div>
          {!railOpen ? (
            <Button
              variant="ghost"
              shape="square"
              aria-label="Show settings"
              onClick={() => setRailOpen(true)}
            >
              <IconAdjustments size={18} />
            </Button>
          ) : null}
        </div>
        <div
          className={`mx-auto flex max-w-5xl flex-col px-4 pb-8 sm:px-6 ${tab === "chat" ? "min-h-screen justify-between" : ""}`}
        >
          {tab === "chat" ? (
            <div className="flex-1 pt-8">
              {!generationModel ? (
                <EmptyState
                  title="Configure a generation model"
                  description="Chat needs a generation model configured for this AI Search instance."
                  action={
                    <LinkButton
                      href="https://open-compute.dev/docs/ocd/configuration/#ai-provider-backends-and-embedding-profiles"
                      external
                    >
                      View configuration docs
                    </LinkButton>
                  }
                />
              ) : submittedChat ? (
                <div className="grid gap-5">
                  <div className="bg-kumo-brand text-kumo-inverse ml-auto max-w-5/6 rounded-xl px-4 py-3 text-sm">
                    {submittedChat}
                  </div>
                  {chat.isPending ? (
                    <p className="text-kumo-subtle text-sm">
                      Generating answer…
                    </p>
                  ) : null}
                  {chatText || chatChunks.length || chat.isSuccess ? (
                    <div className="bg-kumo-base ring-kumo-line rounded-xl px-4 py-3 text-sm ring">
                      <div className="whitespace-pre-wrap">
                        {chatText ||
                          (chat.isPending
                            ? "Generating answer…"
                            : "No answer was returned.")}
                      </div>
                      {chatChunks.length ? (
                        <div className="border-kumo-line mt-3 grid gap-2 border-t pt-2">
                          <Button
                            variant="ghost"
                            className="w-fit text-xs"
                            onClick={() => setSourcesOpen((value) => !value)}
                            aria-expanded={sourcesOpen}
                          >
                            Sources ({chatChunks.length}){" "}
                            <IconCaretDown size={14} />
                          </Button>
                          {sourcesOpen
                            ? chatChunks.map((chunk) => (
                                <ChunkCard
                                  key={chunk.id}
                                  chunk={chunk}
                                  namespaceName={namespaceName}
                                  instanceId={instanceId}
                                  onCopy={copyText}
                                />
                              ))
                            : null}
                        </div>
                      ) : null}
                    </div>
                  ) : null}
                  <Button
                    variant="ghost"
                    className="w-fit"
                    onClick={() => {
                      chatAbort.current?.abort();
                      setSubmittedChat("");
                      setChatText("");
                      setChatChunks([]);
                      chat.reset();
                    }}
                  >
                    Clear conversation
                  </Button>
                </div>
              ) : (
                <div className="flex min-h-80 flex-col items-center justify-center gap-3 text-center">
                  <IconSearch size={30} className="text-kumo-subtle" />
                  <h2 className="text-lg font-semibold">Ask your documents</h2>
                  <p className="text-kumo-subtle text-sm">
                    Try asking a question to start searching for information
                  </p>
                </div>
              )}
            </div>
          ) : null}
          {tab !== "chat" || generationModel ? (
            <form
              className={`bg-kumo-base ring-kumo-line flex gap-2 rounded-2xl p-3 ring ${tab === "search" ? "mt-5" : "mx-auto w-full max-w-2xl"}`}
              onSubmit={(event) => {
                event.preventDefault();
                submit();
              }}
            >
              <input
                aria-label={
                  tab === "search"
                    ? "Search your documents"
                    : "Enter your message"
                }
                className="bg-kumo-control focus-visible:ring-kumo-link min-w-0 flex-1 rounded-r-none px-3 text-sm outline-none focus-visible:ring-2"
                placeholder={
                  tab === "search"
                    ? "Search your documents..."
                    : "Enter your message..."
                }
                value={query}
                onChange={(event) => setQuery(event.target.value)}
              />
              <Button
                className="rounded-l-none"
                type="submit"
                variant="primary"
                shape="square"
                aria-label={tab === "search" ? "Search" : "Send message"}
                disabled={
                  !query.trim() ||
                  pending ||
                  maximum === 0 ||
                  !client ||
                  !selectedInstanceId
                }
              >
                {tab === "search" ? (
                  <IconSearch size={18} />
                ) : (
                  <IconSend size={18} />
                )}
              </Button>
            </form>
          ) : null}
          {maximum === 0 ? (
            <p className="text-kumo-subtle mt-2 text-sm">
              Select at least one maximum result to search.
            </p>
          ) : null}
          {activeError ? (
            <p role="alert" className="text-kumo-danger mt-4 text-sm">
              {activeError instanceof Error
                ? activeError.message
                : "The request failed."}
            </p>
          ) : null}
          {tab === "search" ? (
            <div className="mt-5 grid gap-4">
              {search.isPending ? (
                <p className="text-kumo-subtle text-sm">Searching…</p>
              ) : null}
              {search.data ? (
                search.data.chunks.length ? (
                  search.data.chunks.map((chunk) => (
                    <ChunkCard
                      key={chunk.id}
                      chunk={chunk}
                      namespaceName={namespaceName}
                      instanceId={instanceId}
                      onCopy={copyText}
                    />
                  ))
                ) : (
                  <p className="text-kumo-subtle py-24 text-center text-sm">
                    No results found for “{submittedSearch}”.
                  </p>
                )
              ) : !search.isPending ? (
                <div className="flex min-h-80 flex-col items-center justify-center gap-3 text-center">
                  <IconSearch size={30} className="text-kumo-subtle" />
                  <h2 className="text-lg font-semibold">
                    Search your documents
                  </h2>
                  <p className="text-kumo-subtle text-sm">
                    Try asking a question to start searching for information
                  </p>
                </div>
              ) : null}
            </div>
          ) : null}
        </div>
      </div>
      {railOpen ? (
        <aside
          className="border-kumo-line bg-kumo-base w-full shrink-0 border-t lg:w-1/3 lg:border-t-0 lg:border-l"
          aria-label="Playground settings"
        >
          <div className="flex items-center justify-between px-6 py-4 lg:px-12">
            <span className="font-medium">Settings</span>
            <Button
              variant="ghost"
              shape="square"
              aria-label="Hide settings"
              onClick={() => setRailOpen(false)}
            >
              <IconCaretRight size={17} className="hidden lg:block" />
              <IconCaretLeft size={17} className="lg:hidden" />
            </Button>
          </div>
          <div className="divide-kumo-line divide-y px-6 lg:px-12">
            <div className="flex items-center justify-between gap-4 py-5">
              <div className="grid gap-1">
                <span className="font-medium">Query rewrite</span>
                <span className="text-kumo-subtle text-sm">
                  {generationModel
                    ? "Improve the search query before retrieval."
                    : "Configure a generation model to enable query rewrite."}
                </span>
              </div>
              <Switch
                aria-label="Query rewrite"
                checked={rewrite}
                onCheckedChange={setRewrite}
                disabled={!generationModel}
              />
            </div>
            <div className="flex items-center justify-between gap-4 py-5">
              <div className="grid gap-1">
                <span className="font-medium">Reranking</span>
                <span className="text-kumo-subtle text-sm">
                  {generationModel
                    ? "Reorder results by relevance."
                    : "Configure a generation model to enable reranking."}
                </span>
              </div>
              <Switch
                aria-label="Reranking"
                checked={reranking}
                onCheckedChange={setReranking}
                disabled={!generationModel}
              />
            </div>
            <label className="grid gap-2 py-5 text-sm">
              <span className="flex justify-between gap-3">
                <span className="font-medium">Match threshold</span>
                <span>{threshold.toFixed(1)}</span>
              </span>
              <input
                type="range"
                min="0"
                max="1"
                step="0.1"
                value={threshold}
                onChange={(event) => setThreshold(Number(event.target.value))}
                className="accent-kumo-link w-full"
              />
              <span className="text-kumo-subtle flex justify-between">
                <span>0</span>
                <span>1</span>
              </span>
            </label>
            <label className="grid gap-2 py-5 text-sm">
              <span className="flex justify-between gap-3">
                <span className="font-medium">Maximum results</span>
                <span>{maximum}</span>
              </span>
              <input
                type="range"
                min="0"
                max="50"
                step="1"
                value={maximum}
                onChange={(event) => setMaximum(Number(event.target.value))}
                className="accent-kumo-link w-full"
              />
              <span className="text-kumo-subtle flex justify-between">
                <span>0</span>
                <span>50</span>
              </span>
            </label>
            <div className="flex items-center justify-between gap-3 py-5 text-sm">
              <div className="grid gap-1">
                <span className="font-medium">Metadata filters</span>
                <span className="text-kumo-subtle">
                  {filters.length ? `${filters.length} applied` : "None"}
                </span>
              </div>
              <Button variant="secondary" onClick={beginFilters}>
                Edit
              </Button>
            </div>
            {tab === "chat" ? (
              <>
                <div className="flex items-center justify-between py-5 text-sm">
                  <span className="font-medium">Generation model</span>
                  <span className="text-kumo-subtle truncate pl-3">
                    {generationModel || "Not configured"}
                  </span>
                </div>
                <label className="grid gap-2 py-5 text-sm">
                  <span className="flex justify-between gap-3">
                    <span className="font-medium">Context expansion</span>
                    <span>{contextExpansion}</span>
                  </span>
                  <input
                    type="range"
                    min="0"
                    max="3"
                    step="1"
                    value={contextExpansion}
                    onChange={(event) =>
                      setContextExpansion(Number(event.target.value))
                    }
                    className="accent-kumo-link w-full"
                  />
                  <span className="text-kumo-subtle flex justify-between">
                    <span>0</span>
                    <span>3</span>
                  </span>
                </label>
              </>
            ) : null}
          </div>
        </aside>
      ) : null}
      <Dialog.Root open={filterOpen} onOpenChange={setFilterOpen}>
        <Dialog className="px-6 py-5" size="xl">
          <Dialog.Title>Metadata filters</Dialog.Title>
          <Dialog.Description>
            Show results matching metadata in indexed documents.
          </Dialog.Description>
          <div className="mt-6 grid max-h-[55vh] gap-3 overflow-y-auto">
            {filterDraft.map((row, index) => {
              const field = metadataFields.find(
                (entry) => entry.field_name === row.field,
              );
              return (
                <div key={index} className="flex flex-wrap items-end gap-2">
                  <Select
                    className="min-w-36 flex-1"
                    label="Field"
                    value={row.field}
                    items={metadataFields.map((entry) => ({
                      label: `${entry.field_name} (${entry.data_type})`,
                      value: entry.field_name,
                    }))}
                    onValueChange={(value) =>
                      setFilterDraft((current) =>
                        current.map((entry, at) =>
                          at === index
                            ? { ...entry, field: value ?? "", value: "" }
                            : entry,
                        ),
                      )
                    }
                  />
                  <Select
                    className="min-w-28"
                    label="Operator"
                    value={row.operator}
                    items={[
                      { label: "Equals", value: "$eq" },
                      { label: "Does not equal", value: "$ne" },
                      ...(field?.data_type === "boolean"
                        ? []
                        : [
                            { label: "Less than", value: "$lt" },
                            { label: "Greater than", value: "$gt" },
                          ]),
                    ]}
                    onValueChange={(value) =>
                      setFilterDraft((current) =>
                        current.map((entry, at) =>
                          at === index
                            ? { ...entry, operator: value ?? "$eq" }
                            : entry,
                        ),
                      )
                    }
                  />
                  <div className="min-w-36 flex-1">
                    {field?.data_type === "boolean" ? (
                      <Select
                        label="Value"
                        value={row.value}
                        placeholder="Select"
                        items={[
                          { label: "True", value: "true" },
                          { label: "False", value: "false" },
                        ]}
                        onValueChange={(value) =>
                          setFilterDraft((current) =>
                            current.map((entry, at) =>
                              at === index
                                ? { ...entry, value: value ?? "" }
                                : entry,
                            ),
                          )
                        }
                      />
                    ) : (
                      <input
                        aria-label={`Filter ${index + 1} value`}
                        type={
                          field?.data_type === "datetime"
                            ? "datetime-local"
                            : field?.data_type === "number"
                              ? "number"
                              : "text"
                        }
                        className="bg-kumo-control ring-kumo-line h-9 min-w-0 rounded-md px-2 ring"
                        value={row.value}
                        onChange={(event) =>
                          setFilterDraft((current) =>
                            current.map((entry, at) =>
                              at === index
                                ? { ...entry, value: event.target.value }
                                : entry,
                            ),
                          )
                        }
                      />
                    )}
                  </div>
                  <Button
                    variant="ghost"
                    shape="square"
                    aria-label={`Remove filter ${index + 1}`}
                    onClick={() =>
                      setFilterDraft((current) =>
                        current.filter((_, at) => at !== index),
                      )
                    }
                  >
                    <IconTrash size={16} />
                  </Button>
                </div>
              );
            })}
            {!metadataFields.length ? (
              <p className="text-kumo-subtle text-sm">
                No metadata fields are configured for this instance.
              </p>
            ) : null}
          </div>
          {filterError ? (
            <p role="alert" className="text-kumo-danger mt-3 text-sm">
              {filterError}
            </p>
          ) : null}
          <div className="mt-6 flex flex-wrap items-center justify-between gap-2">
            <div className="flex gap-2">
              <Button
                variant="secondary"
                disabled={
                  !metadataFields.length ||
                  filterDraft.length >= Math.min(10, metadataFields.length)
                }
                onClick={() =>
                  setFilterDraft((current) => [
                    ...current,
                    {
                      field:
                        metadataFields.find(
                          (field) =>
                            !current.some(
                              (row) => row.field === field.field_name,
                            ),
                        )?.field_name ?? metadataFields[0]!.field_name,
                      operator: "$eq",
                      value: "",
                    },
                  ])
                }
              >
                <IconPlus size={16} />
                Add filter
              </Button>
              <Button
                variant="ghost"
                onClick={() => {
                  setFilterDraft([]);
                  setFilterError("");
                }}
              >
                Clear all
              </Button>
            </div>
            <div className="flex gap-2">
              <Button variant="secondary" onClick={() => setFilterOpen(false)}>
                Cancel
              </Button>
              <Button
                variant="primary"
                onClick={() => {
                  if (makeFilters(filterDraft, metadataFields) === null) {
                    setFilterError(
                      "Choose distinct fields and valid values for every filter.",
                    );
                    return;
                  }
                  setFilters(filterDraft);
                  setFilterOpen(false);
                }}
              >
                Apply
              </Button>
            </div>
          </div>
        </Dialog>
      </Dialog.Root>
    </div>
  );
}
