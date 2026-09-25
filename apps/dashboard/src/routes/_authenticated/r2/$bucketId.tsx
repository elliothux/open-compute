import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { Table } from "@cloudflare/kumo/components/table";
import { Tabs } from "@cloudflare/kumo/components/tabs";
import {
  IconFile as FileIcon,
  IconCloudUpload,
  IconFolder,
  IconPlus,
  IconRefresh,
  IconUpload,
} from "@tabler/icons-react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import {
  ErrorState,
  LoadingRows,
  PageHeader,
} from "../../../components/dashboard-page";
import { R2BucketSettings } from "../../../components/r2-bucket-settings";
import {
  R2ObjectDeleteDialog,
  type R2DeleteTarget,
} from "../../../components/r2-object-delete-dialog";
import { R2UploadPanel } from "../../../components/r2-upload-panel";
import { RowActionsMenu } from "../../../components/row-actions-menu";
import { useAuth } from "../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";
import { formatBytes } from "../../../lib/format";
import { r2BucketQuery } from "../../../lib/query-options";

export const Route = createFileRoute("/_authenticated/r2/$bucketId")({
  validateSearch: (
    search: Record<string, unknown>,
  ): { prefix: string; tab?: Tab } => ({
    prefix: typeof search.prefix === "string" ? search.prefix : "",
    ...(search.tab === "settings" ? { tab: "settings" } : {}),
  }),
  loader: ({ context, params }) => {
    const { client, instanceId } = context.auth;
    if (!client || !instanceId) return;
    return context.queryClient.ensureQueryData(
      r2BucketQuery(client, instanceId, params.bucketId),
    );
  },
  component: R2DetailPage,
});

type Tab = "objects" | "settings";

function R2DetailPage() {
  const { bucketId } = Route.useParams();
  const { prefix, tab: searchTab } = Route.useSearch();
  const tab = searchTab ?? "objects";
  const navigate = useNavigate();
  const { client, instanceId: selectedInstanceId } = useAuth();
  const feedback = useMutationFeedback();
  const enabled = client !== null && selectedInstanceId !== null;
  const [searchDraft, setSearchDraft] = useState(prefix);
  const [showFolders, setShowFolders] = useState(true);
  const [folderOpen, setFolderOpen] = useState(false);
  const [folderName, setFolderName] = useState("");
  const [uploadOpen, setUploadOpen] = useState(false);
  const [droppedFiles, setDroppedFiles] = useState<File[]>([]);
  const [selectedKeys, setSelectedKeys] = useState<Set<string>>(new Set());
  const [deleteTargets, setDeleteTargets] = useState<R2DeleteTarget[]>([]);

  const bucket = useQuery(r2BucketQuery(client, selectedInstanceId, bucketId));
  const objects = useQuery({
    queryKey: [
      "cloudflare-v4",
      "r2",
      selectedInstanceId,
      bucketId,
      "objects",
      prefix,
      showFolders,
    ],
    queryFn: ({ signal }) =>
      client!.r2.buckets.objects.list(
        bucketId,
        {
          account_id: selectedInstanceId!,
          ...(prefix.trim() ? { prefix: prefix.trim() } : {}),
          ...(showFolders ? { delimiter: "/" } : {}),
        },
        { signal },
      ),
    enabled,
  });
  const createFolder = useMutation({
    mutationFn: () =>
      client!.r2.buckets.objects.upload(
        `${prefix}${folderName.trim()}/`,
        new Blob([]),
        { account_id: selectedInstanceId!, bucket_name: bucketId },
      ),
    onSuccess: async () => {
      setFolderOpen(false);
      setFolderName("");
      await objects.refetch();
      feedback.success("Folder added.");
    },
    onError: (error) => feedback.failure(error, "Unable to add the folder."),
  });
  const remove = useMutation({
    mutationFn: async () => {
      const keys = new Set<string>();
      for (const target of deleteTargets) {
        if (!target.folder) {
          keys.add(target.key);
          continue;
        }
        let cursor: string | undefined;
        const seen = new Set<string>();
        do {
          const page = await client!.r2.buckets.objects.list(bucketId, {
            account_id: selectedInstanceId!,
            prefix: target.key,
            per_page: 1000,
            ...(cursor ? { cursor } : {}),
          });
          for (const object of page.result) {
            if (!object.key || !object.key.startsWith(target.key)) {
              throw new Error("Folder listing returned an invalid object key.");
            }
            keys.add(object.key);
          }
          const next = page.result_info.cursor;
          if (next && seen.has(next)) {
            throw new Error("Folder listing returned a repeated cursor.");
          }
          if (next) seen.add(next);
          if (
            "is_truncated" in page.result_info &&
            page.result_info.is_truncated === true &&
            !next
          ) {
            throw new Error(
              "Folder listing ended before all objects were returned.",
            );
          }
          cursor = next || undefined;
        } while (cursor);
      }
      for (const key of keys) {
        await client!.r2.buckets.objects.delete(key, {
          account_id: selectedInstanceId!,
          bucket_name: bucketId,
        });
      }
    },
    onSuccess: async () => {
      setDeleteTargets([]);
      setSelectedKeys(new Set());
      await objects.refetch();
      feedback.success("Objects deleted.");
    },
    onError: async (error) => {
      await objects.refetch();
      feedback.failure(error, "Unable to delete the objects.");
    },
  });
  const download = useMutation({
    mutationFn: async (key: string) => {
      const response = await client!.r2.buckets.objects.get(key, {
        account_id: selectedInstanceId!,
        bucket_name: bucketId,
      });
      const url = URL.createObjectURL(await response.blob());
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = key.split("/").pop() || "download";
      anchor.click();
      URL.revokeObjectURL(url);
    },
    onError: (error) =>
      feedback.failure(error, "Unable to download the object."),
  });
  const objectRows = objects.data?.result ?? [];
  const resultInfo = objects.data?.result_info;
  const folders =
    resultInfo &&
    "delimited" in resultInfo &&
    Array.isArray(resultInfo.delimited)
      ? resultInfo.delimited.filter(
          (value): value is string => typeof value === "string",
        )
      : [];
  const rows = [
    ...folders.map((key) => ({ key, folder: true as const })),
    ...objectRows
      .filter((item) => !item.key || !folders.includes(item.key))
      .map((item) => ({ ...item, folder: item.key?.endsWith("/") ?? false })),
  ];
  const selectedRows = rows.filter(
    (row): row is typeof row & { key: string } =>
      !!row.key && selectedKeys.has(row.key),
  );
  const selectedFiles = selectedRows.filter((row) => !row.folder).length;
  const selectedFolders = selectedRows.length - selectedFiles;
  const selectedLabel = [
    selectedFolders
      ? `${selectedFolders} ${selectedFolders === 1 ? "folder" : "folders"}`
      : "",
    selectedFiles
      ? `${selectedFiles} ${selectedFiles === 1 ? "file" : "files"}`
      : "",
  ]
    .filter(Boolean)
    .join(" and ");
  const totalSize = objectRows.reduce((sum, item) => sum + (item.size ?? 0), 0);
  const folderValid =
    folderName.trim().length > 0 &&
    !folderName.includes("/") &&
    ![".", ".."].includes(folderName.trim());
  const openPrefix = (value: string) => {
    setSearchDraft(value);
    setSelectedKeys(new Set());
    void navigate({
      to: "/r2/$bucketId",
      params: { bucketId },
      search: { prefix: value },
    });
  };
  const prepareDroppedFiles = (files: FileList) => {
    if (!files.length) return;
    setDroppedFiles(Array.from(files));
    setUploadOpen(true);
  };

  return (
    <div>
      <Dialog.Root open={folderOpen} onOpenChange={setFolderOpen}>
        <Dialog className="px-6 py-5" size="lg">
          <Dialog.Title>Add folder</Dialog.Title>
          <Dialog.Description>
            R2 stores objects in a flat structure. A folder is a zero-byte
            object whose key ends with a slash.
          </Dialog.Description>
          <form
            className="mt-5"
            onSubmit={(event) => {
              event.preventDefault();
              if (folderValid && !createFolder.isPending) createFolder.mutate();
            }}
          >
            <Input
              label="Folder name"
              value={folderName}
              onChange={(event) => setFolderName(event.target.value)}
              autoFocus
            />
            {createFolder.error ? (
              <ErrorState error={createFolder.error} />
            ) : null}
            <div className="mt-6 flex justify-end gap-2">
              <Button
                type="button"
                variant="secondary"
                onClick={() => setFolderOpen(false)}
              >
                Cancel
              </Button>
              <Button
                type="submit"
                variant="primary"
                disabled={!folderValid || createFolder.isPending}
              >
                Add folder
              </Button>
            </div>
          </form>
        </Dialog>
      </Dialog.Root>
      <R2ObjectDeleteDialog
        open={deleteTargets.length > 0}
        bucketName={bucketId}
        targets={deleteTargets}
        pending={remove.isPending}
        error={remove.error}
        onOpenChange={(open) => !open && setDeleteTargets([])}
        onConfirm={() => remove.mutate()}
      />
      <PageHeader title={bucketId} description="R2 bucket" />
      <nav className="mb-5" aria-label="R2 bucket tabs">
        <Tabs
          value={tab}
          tabs={[
            { value: "objects", label: "Objects" },
            { value: "settings", label: "Settings" },
          ]}
          onValueChange={(value) =>
            void navigate({
              to: "/r2/$bucketId",
              params: { bucketId },
              search: {
                prefix,
                ...(value === "objects" ? {} : { tab: value as Tab }),
              },
            })
          }
        />
      </nav>
      {bucket.isLoading ? (
        <LoadingRows />
      ) : bucket.error ? (
        <ErrorState error={bucket.error} />
      ) : tab === "objects" ? (
        <div className="grid min-w-0 gap-5">
          <dl className="border-kumo-line grid min-w-0 grid-cols-2 gap-4 border-b pb-5">
            {[
              ["Listed entries", String(rows.length)],
              ["Listed size", formatBytes(totalSize)],
            ].map(([label, value]) => (
              <div key={label}>
                <dt className="text-kumo-subtle text-xs">{label}</dt>
                <dd className="mt-1 font-medium">{value}</dd>
              </div>
            ))}
          </dl>
          <form
            className="grid gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              openPrefix(searchDraft.trim());
            }}
          >
            <label className="font-medium" htmlFor="r2-object-prefix">
              Search by object prefix
            </label>
            <div className="flex gap-2">
              <div className="w-full max-w-72">
                <Input
                  id="r2-object-prefix"
                  aria-label="Object prefix"
                  placeholder="Search by object prefix"
                  value={searchDraft}
                  onChange={(event) => setSearchDraft(event.target.value)}
                />
              </div>
              <Button type="submit" variant="secondary">
                Search
              </Button>
            </div>
          </form>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={showFolders}
              onChange={(event) => {
                setShowFolders(event.target.checked);
                setSelectedKeys(new Set());
              }}
            />
            Show prefixes as folders
          </label>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex min-w-0 items-center gap-1 text-sm">
              <h1 className="truncate font-medium">
                {bucket.data?.name ?? bucketId}
              </h1>
              <span>/</span>
              {prefix ? (
                <button
                  type="button"
                  className="truncate hover:underline"
                  onClick={() => openPrefix("")}
                >
                  {prefix}
                </button>
              ) : null}
            </div>
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              {selectedRows.length ? (
                <Button
                  variant="destructive"
                  onClick={() =>
                    setDeleteTargets(
                      selectedRows.map(({ key, folder }) => ({ key, folder })),
                    )
                  }
                >
                  Delete {selectedLabel}
                </Button>
              ) : null}
              {!uploadOpen ? (
                <Button variant="secondary" onClick={() => setUploadOpen(true)}>
                  <IconUpload size={16} />
                  Upload
                </Button>
              ) : null}
              <Button variant="primary" onClick={() => setFolderOpen(true)}>
                <IconPlus size={16} />
                Add folder
              </Button>
              <Button
                variant="secondary"
                shape="square"
                aria-label="Refresh objects"
                onClick={() => void objects.refetch()}
              >
                <IconRefresh size={16} />
              </Button>
            </div>
          </div>
          {uploadOpen ? (
            <R2UploadPanel
              bucketId={bucketId}
              prefix={prefix}
              initialFiles={droppedFiles}
              onInitialFilesHandled={() => setDroppedFiles([])}
              onClose={() => setUploadOpen(false)}
              onUploaded={async () => {
                await objects.refetch();
              }}
            />
          ) : null}
          <div className="min-w-0">
            {objects.isLoading ? (
              <LoadingRows />
            ) : objects.error ? (
              <ErrorState error={objects.error} />
            ) : (
              <LayerCard className="min-w-0 overflow-hidden p-0">
                <div className="max-w-full overflow-x-auto">
                  <Table layout="fixed" className="sm:table-auto">
                    <Table.Header variant="compact">
                      <Table.Row>
                        <Table.Head className="w-8 sm:w-10">
                          <input
                            type="checkbox"
                            aria-label="Select all listed objects"
                            checked={
                              rows.length > 0 &&
                              selectedRows.length === rows.length
                            }
                            ref={(node) => {
                              if (node)
                                node.indeterminate =
                                  selectedRows.length > 0 &&
                                  selectedRows.length < rows.length;
                            }}
                            onChange={(event) =>
                              setSelectedKeys(
                                event.target.checked
                                  ? new Set(
                                      rows.flatMap((row) =>
                                        row.key ? [row.key] : [],
                                      ),
                                    )
                                  : new Set(),
                              )
                            }
                          />
                        </Table.Head>
                        <Table.Head className="w-28 sm:w-auto">
                          Object
                        </Table.Head>
                        <Table.Head className="w-12 sm:w-auto">Type</Table.Head>
                        <Table.Head className="w-10 sm:w-auto">
                          Storage class
                        </Table.Head>
                        <Table.Head className="w-8 sm:w-auto">Size</Table.Head>
                        <Table.Head className="w-10 sm:w-auto">
                          Modified
                        </Table.Head>
                        <Table.Head className="w-11 sm:w-12">
                          <span className="sr-only">Actions</span>
                        </Table.Head>
                      </Table.Row>
                    </Table.Header>
                    <Table.Body>
                      {rows.length === 0 ? (
                        <Table.Row>
                          <Table.Cell colSpan={7}>
                            <div
                              className="border-kumo-line flex min-h-60 flex-wrap items-center justify-center gap-10 rounded-md border border-dashed px-5 py-8 text-center sm:text-left"
                              onDragOver={(event) => event.preventDefault()}
                              onDrop={(event) => {
                                event.preventDefault();
                                prepareDroppedFiles(event.dataTransfer.files);
                              }}
                            >
                              <IconCloudUpload
                                size={96}
                                strokeWidth={1.5}
                                className="text-kumo-subtle"
                              />
                              <div className="grid gap-2">
                                <p className="font-medium">
                                  {prefix
                                    ? "No objects match this prefix"
                                    : "Your bucket is ready. Add files to get started."}
                                </p>
                                <p className="text-kumo-subtle">
                                  Drag and drop a file or{" "}
                                  <button
                                    type="button"
                                    className="text-kumo-brand hover:underline"
                                    onClick={() => setUploadOpen(true)}
                                  >
                                    choose from your computer
                                  </button>
                                  .
                                </p>
                                <p className="text-kumo-subtle">
                                  Large files use multipart upload.
                                </p>
                              </div>
                            </div>
                          </Table.Cell>
                        </Table.Row>
                      ) : null}
                      {rows.map((object) => (
                        <Table.Row key={object.key}>
                          <Table.Cell>
                            <input
                              type="checkbox"
                              aria-label={`Select ${object.key}`}
                              checked={
                                !!object.key && selectedKeys.has(object.key)
                              }
                              onChange={(event) => {
                                if (!object.key) return;
                                setSelectedKeys((current) => {
                                  const next = new Set(current);
                                  if (event.target.checked)
                                    next.add(object.key!);
                                  else next.delete(object.key!);
                                  return next;
                                });
                              }}
                            />
                          </Table.Cell>
                          <Table.Cell>
                            <button
                              type="button"
                              className={`inline-flex max-w-full min-w-0 items-center gap-2 font-mono text-xs hover:underline ${object.folder ? "text-kumo-brand" : ""}`}
                              onClick={() =>
                                object.key &&
                                (object.folder
                                  ? openPrefix(object.key)
                                  : void navigate({
                                      to: "/r2/$bucketId/objects/$objectKey/details",
                                      params: {
                                        bucketId,
                                        objectKey: object.key,
                                      },
                                    }))
                              }
                            >
                              {object.folder ? (
                                <IconFolder
                                  size={16}
                                  className="text-kumo-brand"
                                />
                              ) : (
                                <FileIcon
                                  size={16}
                                  className="text-kumo-brand"
                                />
                              )}
                              <span className="min-w-0 text-left break-all">
                                {object.key?.slice(prefix.length) ||
                                  object.key ||
                                  "Unnamed object"}
                              </span>
                            </button>
                          </Table.Cell>
                          <Table.Cell className="text-kumo-subtle break-all">
                            {object.folder
                              ? "Folder"
                              : "http_metadata" in object
                                ? object.http_metadata?.contentType || "—"
                                : "—"}
                          </Table.Cell>
                          <Table.Cell className="text-kumo-subtle break-words">
                            {"storage_class" in object
                              ? (object.storage_class ?? "Standard")
                              : "—"}
                          </Table.Cell>
                          <Table.Cell>
                            {"size" in object ? formatBytes(object.size) : "—"}
                          </Table.Cell>
                          <Table.Cell className="text-kumo-subtle break-words">
                            {"last_modified" in object && object.last_modified
                              ? new Date(object.last_modified).toLocaleString()
                              : "—"}
                          </Table.Cell>
                          <Table.Cell>
                            {object.key ? (
                              <RowActionsMenu
                                label={object.key}
                                actions={
                                  object.folder
                                    ? [
                                        {
                                          id: "delete",
                                          label: "Delete folder",
                                          variant: "danger",
                                          onSelect: () =>
                                            setDeleteTargets([
                                              {
                                                key: object.key!,
                                                folder: true,
                                              },
                                            ]),
                                        },
                                      ]
                                    : [
                                        {
                                          id: "download",
                                          label: "Download",
                                          onSelect: () =>
                                            download.mutate(object.key!),
                                          disabled: download.isPending,
                                        },
                                        {
                                          id: "delete",
                                          label: "Delete",
                                          variant: "danger",
                                          onSelect: () =>
                                            setDeleteTargets([
                                              {
                                                key: object.key!,
                                                folder: false,
                                              },
                                            ]),
                                        },
                                      ]
                                }
                              />
                            ) : null}
                          </Table.Cell>
                        </Table.Row>
                      ))}
                    </Table.Body>
                  </Table>
                </div>
              </LayerCard>
            )}
            {!objects.isLoading && !objects.error && rows.length > 0 ? (
              <div
                className="text-kumo-subtle mt-3 flex min-h-12 items-center justify-center gap-2 text-sm"
                onDragOver={(event) => event.preventDefault()}
                onDrop={(event) => {
                  event.preventDefault();
                  prepareDroppedFiles(event.dataTransfer.files);
                }}
              >
                <IconCloudUpload size={16} /> Drop a file here to upload
              </div>
            ) : null}
          </div>
        </div>
      ) : (
        <R2BucketSettings
          name={bucket.data?.name ?? bucketId}
          creationDate={bucket.data?.creation_date}
        />
      )}
    </div>
  );
}
