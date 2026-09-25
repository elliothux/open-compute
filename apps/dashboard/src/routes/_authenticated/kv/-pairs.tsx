import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { Popover } from "@cloudflare/kumo/components/popover";
import { Table } from "@cloudflare/kumo/components/table";
import {
  IconCaretDown,
  IconCaretRight,
  IconCopy,
  IconDots,
} from "@tabler/icons-react";
import {
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { Fragment, useState } from "react";
import {
  CatalogToolbar,
  EmptyState,
  ErrorState,
  LoadingRows,
} from "../../../components/dashboard-page";
import { openConfirmDeleteDialog } from "../../../components/resource-dialog";
import { useAuth } from "../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";

export function KvPairs({ namespaceId }: { namespaceId: string }) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const feedback = useMutationFeedback();
  const queryClient = useQueryClient();
  const enabled = client !== null && selectedInstanceId !== null;
  const [prefix, setPrefix] = useState("");
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [editedValue, setEditedValue] = useState("");
  const [menuKey, setMenuKey] = useState<string | null>(null);
  const [draftKey, setDraftKey] = useState("");
  const [draftValue, setDraftValue] = useState("");
  const keys = useInfiniteQuery({
    queryKey: [
      "cloudflare-v4",
      "kv",
      selectedInstanceId,
      namespaceId,
      "keys",
      prefix,
    ],
    queryFn: async ({ signal, pageParam }) => {
      const page = await client!.kv.namespaces.keys.list(
        namespaceId,
        {
          account_id: selectedInstanceId!,
          ...(prefix.trim() ? { prefix: prefix.trim() } : {}),
          ...(pageParam ? { cursor: pageParam } : {}),
          limit: 100,
        },
        { signal },
      );
      const names = page.result.map((key) => key.name);
      const response = names.length
        ? await client!.kv.namespaces.bulkGet(
            namespaceId,
            { account_id: selectedInstanceId!, keys: names, type: "text" },
            { signal },
          )
        : null;
      return {
        rows: page.result.map((key) => ({
          ...key,
          value: response?.values?.[key.name],
        })),
        cursor: page.result_info.cursor || undefined,
      };
    },
    initialPageParam: "",
    getNextPageParam: (page) => page.cursor,
    enabled,
  });
  const value = useQuery({
    queryKey: [
      "cloudflare-v4",
      "kv",
      selectedInstanceId,
      namespaceId,
      "value",
      selectedKey,
    ],
    queryFn: async ({ signal }) =>
      (
        await client!.kv.namespaces.values.get(
          selectedKey!,
          { account_id: selectedInstanceId!, namespace_id: namespaceId },
          { signal },
        )
      ).text(),
    enabled: enabled && selectedKey !== null,
  });
  const put = useMutation({
    mutationFn: () =>
      client!.kv.namespaces.values.update(draftKey.trim(), {
        account_id: selectedInstanceId!,
        namespace_id: namespaceId,
        value: draftValue,
      }),
    onSuccess: async () => {
      setDraftKey("");
      setDraftValue("");
      await queryClient.invalidateQueries({
        queryKey: [
          "cloudflare-v4",
          "kv",
          selectedInstanceId,
          namespaceId,
          "keys",
        ],
      });
      feedback.success("KV pair added.");
    },
    onError: (error) => feedback.failure(error, "Unable to add the KV pair."),
  });
  const edit = useMutation({
    mutationFn: () => {
      const key = keys.data?.pages
        .flatMap((page) => page.rows)
        .find((row) => row.name === selectedKey);
      return client!.kv.namespaces.values.update(selectedKey!, {
        account_id: selectedInstanceId!,
        namespace_id: namespaceId,
        value: editedValue,
        ...(key?.metadata === undefined ? {} : { metadata: key.metadata }),
        ...(key?.expiration === undefined
          ? {}
          : { expiration: key.expiration }),
      });
    },
    onSuccess: async () => {
      setEditing(false);
      await Promise.all([
        queryClient.invalidateQueries({
          queryKey: [
            "cloudflare-v4",
            "kv",
            selectedInstanceId,
            namespaceId,
            "keys",
          ],
        }),
        queryClient.invalidateQueries({
          queryKey: [
            "cloudflare-v4",
            "kv",
            selectedInstanceId,
            namespaceId,
            "value",
            selectedKey,
          ],
        }),
      ]);
      feedback.success("KV pair saved.");
    },
    onError: (error) => feedback.failure(error, "Unable to save the KV pair."),
  });
  function confirmDeleteKey(key: string) {
    openConfirmDeleteDialog({
      name: key,
      confirm: async () => {
        try {
          await client!.kv.namespaces.values.delete(key, {
            account_id: selectedInstanceId!,
            namespace_id: namespaceId,
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the KV pair.");
          throw error;
        }
        if (selectedKey === key) setSelectedKey(null);
        await queryClient.invalidateQueries({
          queryKey: [
            "cloudflare-v4",
            "kv",
            selectedInstanceId,
            namespaceId,
            "keys",
          ],
        });
        feedback.success("KV pair deleted.");
      },
    });
  }

  const rows = keys.data?.pages.flatMap((page) => page.rows) ?? [];
  const copyText = (text: string) => {
    void navigator.clipboard.writeText(text).then(
      () => feedback.success("Copied to clipboard."),
      (error: unknown) =>
        feedback.failure(error, "Unable to copy to clipboard."),
    );
  };
  return (
    <>
      <div className="grid min-w-0 gap-4 text-sm">
        <CatalogToolbar
          value={prefix}
          onChange={(value) => {
            setPrefix(value);
            setSelectedKey(null);
            setEditing(false);
          }}
          onRefresh={() => void keys.refetch()}
          refreshing={keys.isFetching}
          placeholder="Search keys by prefix"
        />
        <form
          className="grid items-end gap-2 sm:grid-cols-3"
          onSubmit={(event) => {
            event.preventDefault();
            if (draftKey.trim() && !put.isPending) put.mutate();
          }}
        >
          <Input
            label="Key"
            value={draftKey}
            onChange={(event) => setDraftKey(event.target.value)}
          />
          <Input
            label="Value"
            value={draftValue}
            onChange={(event) => setDraftValue(event.target.value)}
          />
          <Button
            type="submit"
            variant="primary"
            disabled={!draftKey.trim() || put.isPending}
          >
            {put.isPending ? "Adding…" : "Add entry"}
          </Button>
        </form>
        {put.error ? <ErrorState error={put.error} /> : null}
        {keys.isLoading ? (
          <LoadingRows count={3} />
        ) : keys.error ? (
          <ErrorState error={keys.error} />
        ) : rows.length === 0 ? (
          <EmptyState
            title={prefix ? "No matching keys" : "No KV pairs"}
            description={
              prefix
                ? "Try a different key prefix."
                : "Add a key and value above to populate this namespace."
            }
          />
        ) : (
          <LayerCard className="min-w-0 overflow-hidden p-0">
            <div className="overflow-x-auto">
              <Table layout="fixed" className="min-w-0 sm:min-w-2xl">
                <colgroup>
                  <col className="w-16" />
                  <col className="w-1/3" />
                  <col />
                  <col className="w-12" />
                </colgroup>
                <Table.Header variant="compact">
                  <Table.Row className="h-11">
                    <Table.Head></Table.Head>
                    <Table.Head>Key</Table.Head>
                    <Table.Head>Value</Table.Head>
                    <Table.Head>
                      <span className="sr-only">Actions</span>
                    </Table.Head>
                  </Table.Row>
                </Table.Header>
                <Table.Body>
                  {rows.map((key) => (
                    <Fragment key={key.name}>
                      <Table.Row className="h-11">
                        <Table.Cell>
                          <Button
                            variant="ghost"
                            shape="square"
                            size="sm"
                            aria-label={`${selectedKey === key.name ? "Collapse" : "Expand"} ${key.name}`}
                            onClick={() => {
                              setSelectedKey(
                                selectedKey === key.name ? null : key.name,
                              );
                              setEditing(false);
                            }}
                          >
                            {selectedKey === key.name ? (
                              <IconCaretDown size={14} />
                            ) : (
                              <IconCaretRight size={14} />
                            )}
                          </Button>
                        </Table.Cell>
                        <Table.Cell className="truncate">{key.name}</Table.Cell>
                        <Table.Cell className="truncate">
                          {displayValue(key.value)}
                        </Table.Cell>
                        <Table.Cell className="text-right">
                          <Popover
                            open={menuKey === key.name}
                            onOpenChange={(open) =>
                              setMenuKey(open ? key.name : null)
                            }
                          >
                            <Popover.Trigger
                              render={
                                <Button
                                  variant="ghost"
                                  shape="square"
                                  size="sm"
                                  aria-label={`More actions for ${key.name}`}
                                />
                              }
                            >
                              <IconDots size={18} />
                            </Popover.Trigger>
                            <Popover.Content className="w-36 p-1">
                              <Popover.Title className="sr-only">
                                Key actions
                              </Popover.Title>
                              <button
                                type="button"
                                className="text-kumo-danger hover:bg-kumo-tint w-full rounded-md px-3 py-2 text-left text-sm"
                                onClick={() => {
                                  setMenuKey(null);
                                  confirmDeleteKey(key.name);
                                }}
                              >
                                Delete
                              </button>
                            </Popover.Content>
                          </Popover>
                        </Table.Cell>
                      </Table.Row>
                      {selectedKey === key.name ? (
                        <Table.Row variant="selected">
                          <Table.Cell colSpan={4}>
                            {value.isLoading ? (
                              <LoadingRows count={1} />
                            ) : value.error ? (
                              <ErrorState error={value.error} />
                            ) : (
                              <div className="grid gap-4">
                                <div className="flex flex-wrap justify-end gap-2">
                                  {editing ? (
                                    <>
                                      <label className="relative">
                                        <input
                                          className="absolute h-px w-px opacity-0"
                                          type="file"
                                          aria-label="Upload value"
                                          onChange={(event) => {
                                            const file =
                                              event.target.files?.[0];
                                            if (file)
                                              void file
                                                .text()
                                                .then(setEditedValue);
                                          }}
                                        />
                                        <span className="cursor-pointer">
                                          Upload value…
                                        </span>
                                      </label>
                                      <Button
                                        variant="primary"
                                        size="sm"
                                        disabled={edit.isPending}
                                        onClick={() => edit.mutate()}
                                      >
                                        {edit.isPending ? "Saving…" : "Save"}
                                      </Button>
                                    </>
                                  ) : (
                                    <>
                                      <Button
                                        variant="ghost"
                                        size="sm"
                                        onClick={() => {
                                          setEditedValue(value.data ?? "");
                                          setEditing(true);
                                        }}
                                      >
                                        Edit
                                      </Button>
                                      <Button
                                        variant="ghost"
                                        size="sm"
                                        onClick={() =>
                                          downloadValue(
                                            key.name,
                                            value.data ?? "",
                                          )
                                        }
                                      >
                                        Download
                                      </Button>
                                    </>
                                  )}
                                  <Button
                                    variant="ghost"
                                    size="sm"
                                    onClick={() => {
                                      setEditing(false);
                                      setSelectedKey(null);
                                    }}
                                  >
                                    Cancel
                                  </Button>
                                </div>
                                <div className="grid gap-3 sm:grid-cols-2">
                                  <div className="relative grid gap-1">
                                    <span className="font-medium">Key</span>
                                    <textarea
                                      aria-label="Selected key"
                                      className="border-kumo-line bg-kumo-control h-32 w-full resize-y rounded-md border px-3 py-2 outline-none"
                                      readOnly
                                      value={key.name}
                                    />
                                    <Button
                                      variant="ghost"
                                      shape="square"
                                      size="sm"
                                      className="absolute top-7 right-1"
                                      aria-label={`Copy key ${key.name}`}
                                      onClick={() => copyText(key.name)}
                                    >
                                      <IconCopy size={14} />
                                    </Button>
                                  </div>
                                  <div className="relative grid gap-1">
                                    <span className="font-medium">Value</span>
                                    <textarea
                                      aria-label="Selected value"
                                      className="border-kumo-line bg-kumo-control h-32 w-full resize-y rounded-md border px-3 py-2 outline-none"
                                      readOnly={!editing}
                                      value={
                                        editing
                                          ? editedValue
                                          : (value.data ?? "")
                                      }
                                      onChange={(event) =>
                                        setEditedValue(event.target.value)
                                      }
                                    />
                                    <Button
                                      variant="ghost"
                                      shape="square"
                                      size="sm"
                                      className="absolute top-7 right-1"
                                      aria-label={`Copy value for ${key.name}`}
                                      onClick={() =>
                                        copyText(
                                          editing
                                            ? editedValue
                                            : (value.data ?? ""),
                                        )
                                      }
                                    >
                                      <IconCopy size={14} />
                                    </Button>
                                  </div>
                                </div>
                                {edit.error ? (
                                  <ErrorState error={edit.error} />
                                ) : null}
                              </div>
                            )}
                          </Table.Cell>
                        </Table.Row>
                      ) : null}
                    </Fragment>
                  ))}
                </Table.Body>
              </Table>
            </div>
          </LayerCard>
        )}
        {keys.hasNextPage ? (
          <div className="flex justify-center">
            <Button
              variant="secondary"
              disabled={keys.isFetchingNextPage}
              onClick={() => void keys.fetchNextPage()}
            >
              {keys.isFetchingNextPage ? "Loading…" : "Load more"}
            </Button>
          </div>
        ) : null}
      </div>
    </>
  );
}

function displayValue(value: unknown): string {
  return typeof value === "string" ? value : "—";
}

function downloadValue(key: string, value: string) {
  const url = URL.createObjectURL(
    new Blob([value], { type: "text/plain;charset=utf-8" }),
  );
  const link = document.createElement("a");
  link.href = url;
  link.download = key.split("/").pop() || "value.txt";
  link.click();
  URL.revokeObjectURL(url);
}
