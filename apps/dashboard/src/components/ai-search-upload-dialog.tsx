import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import {
  IconCircleCheck,
  IconFolder,
  IconUpload,
  IconX,
} from "@tabler/icons-react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { useAuth } from "../features/auth/auth-atoms";
import { useMutationFeedback } from "../features/toast/use-mutation-feedback";

const maxFiles = 100;
const maxFileBytes = 4 * 1024 * 1024;

async function droppedFiles(entries: FileSystemEntry[]) {
  const files: File[] = [];
  async function collect(
    entry: FileSystemEntry,
    prefix: string,
  ): Promise<void> {
    if (entry.isFile) {
      const file = await new Promise<File>((resolve, reject) =>
        (entry as FileSystemFileEntry).file(resolve, reject),
      );
      files.push(
        new File([file], `${prefix}${file.name}`, {
          type: file.type,
          lastModified: file.lastModified,
        }),
      );
      if (files.length > maxFiles)
        throw new Error(`Choose up to ${maxFiles} files.`);
      return;
    }
    const reader = (entry as FileSystemDirectoryEntry).createReader();
    for (;;) {
      const batch = await new Promise<FileSystemEntry[]>((resolve, reject) =>
        reader.readEntries(resolve, reject),
      );
      if (batch.length === 0) return;
      for (const child of batch)
        await collect(child, `${prefix}${entry.name}/`);
    }
  }
  for (const entry of entries) await collect(entry, "");
  return files;
}

export function AISearchUploadDialog({
  open,
  onOpenChange,
  namespaceName,
  instanceId,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  namespaceName: string;
  instanceId: string;
}) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const fileRef = useRef<HTMLInputElement>(null);
  const folderRef = useRef<HTMLInputElement>(null);
  const [files, setFiles] = useState<File[]>([]);
  const filesRef = useRef<File[]>([]);
  const [statuses, setStatuses] = useState<
    ("pending" | "success" | "failed")[]
  >([]);
  const [error, setError] = useState("");
  const replaceFiles = (next: File[]) => {
    filesRef.current = next;
    setFiles(next);
    setStatuses(next.map(() => "pending"));
  };
  const removeFile = (index: number) => {
    filesRef.current = filesRef.current.filter((_, at) => at !== index);
    setFiles(filesRef.current);
    setStatuses((current) => current.filter((_, at) => at !== index));
  };
  const clearCompleted = () => {
    filesRef.current = filesRef.current.filter(
      (_, index) => statuses[index] !== "success",
    );
    setFiles(filesRef.current);
    setStatuses((current) => current.filter((status) => status !== "success"));
  };

  const resetAndClose = () => {
    replaceFiles([]);
    setError("");
    if (fileRef.current) fileRef.current.value = "";
    if (folderRef.current) folderRef.current.value = "";
    onOpenChange(false);
  };
  const close = () => {
    if (!upload.isPending) resetAndClose();
  };
  const addFiles = (candidates: File[]) => {
    if (candidates.length === 0) return;
    if (filesRef.current.length + candidates.length > maxFiles) {
      setError(`Choose up to ${maxFiles} files.`);
      return;
    }
    const invalid = candidates.find(
      (file) => file.size === 0 || file.size > maxFileBytes,
    );
    if (invalid) {
      setError(`${invalid.name} must be nonempty and no larger than 4 MB.`);
      return;
    }
    const badPath = candidates.find(
      (file) =>
        [...file.name].length > 128 ||
        file.name
          .split("/")
          .some((part) => !part || part === "." || part === ".."),
    );
    if (badPath) {
      setError(`${badPath.name} has an invalid or overly long path.`);
      return;
    }
    filesRef.current = [...filesRef.current, ...candidates];
    setFiles(filesRef.current);
    setStatuses((current) => [
      ...current,
      ...candidates.map(() => "pending" as const),
    ]);
    setError("");
  };
  const uploaded = statuses.filter((status) => status === "success").length;
  const failed = statuses.filter((status) => status === "failed").length;
  const upload = useMutation({
    mutationFn: async () => {
      const total = files.length;
      let completed = 0;
      for (const [index, file] of files.entries()) {
        if (statuses[index] === "success") {
          completed += 1;
          continue;
        }
        try {
          await client!.aiSearch.namespaces.instances.items.upload(instanceId, {
            account_id: selectedInstanceId!,
            name: namespaceName,
            file: { file },
          });
        } catch {
          setStatuses((current) =>
            current.map((status, at) => (at === index ? "failed" : status)),
          );
          throw new Error(
            `${completed} of ${total} files uploaded. The remaining files were not uploaded.`,
          );
        }
        completed += 1;
        setStatuses((current) =>
          current.map((status, at) => (at === index ? "success" : status)),
        );
      }
      return completed;
    },
    onSuccess: (count) => {
      feedback.success(
        `${count} ${count === 1 ? "file" : "files"} uploaded for indexing.`,
      );
      setError("");
    },
    onError: (failure) =>
      setError(failure instanceof Error ? failure.message : "Upload failed."),
    onSettled: async () => {
      await queryClient.invalidateQueries({
        queryKey: [
          "ai-search",
          selectedInstanceId,
          namespaceName,
          instanceId,
          "items",
        ],
      });
    },
  });

  return (
    <Dialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) close();
        else onOpenChange(true);
      }}
    >
      <Dialog size="xl" className="px-5 py-4">
        <Dialog.Title className="text-xl font-semibold">
          Upload files
        </Dialog.Title>
        <Dialog.Description>
          For automatic uploads, use the API or a Worker binding.
        </Dialog.Description>
        <input
          ref={fileRef}
          type="file"
          multiple
          className="sr-only"
          aria-label="Choose files"
          onChange={(event) => {
            addFiles(Array.from(event.target.files ?? []));
            event.target.value = "";
          }}
        />
        <input
          ref={(element) => {
            folderRef.current = element;
            element?.setAttribute("webkitdirectory", "");
          }}
          type="file"
          multiple
          className="sr-only"
          aria-label="Choose a folder"
          onChange={(event) => {
            addFiles(
              Array.from(
                event.target.files ?? [],
                (file) =>
                  new File([file], file.webkitRelativePath || file.name, {
                    type: file.type,
                    lastModified: file.lastModified,
                  }),
              ),
            );
            event.target.value = "";
          }}
        />
        <div
          aria-label="Drop files or folders"
          className="border-kumo-line mt-5 flex min-h-40 flex-col items-center justify-center gap-2 rounded-lg border border-dashed px-4 py-5 text-center"
          onDragOver={(event) => event.preventDefault()}
          onDrop={(event) => {
            event.preventDefault();
            if (upload.isPending) return;
            const entries = Array.from(event.dataTransfer.items, (item) =>
              item.webkitGetAsEntry(),
            ).filter((entry): entry is FileSystemEntry => entry !== null);
            if (entries.length === 0) {
              addFiles(Array.from(event.dataTransfer.files));
              return;
            }
            void droppedFiles(entries)
              .then(addFiles)
              .catch((failure: unknown) =>
                setError(
                  failure instanceof Error
                    ? failure.message
                    : "Unable to read the dropped folder.",
                ),
              );
          }}
        >
          <IconUpload size={28} className="text-kumo-subtle" />
          <div className="flex flex-wrap items-center justify-center gap-1 text-sm">
            <span>Drag files here, or</span>
            <Button
              variant="ghost"
              className="text-kumo-link"
              disabled={upload.isPending}
              onClick={() => fileRef.current?.click()}
            >
              Choose files
            </Button>
            <span>or</span>
            <Button
              variant="ghost"
              className="text-kumo-link"
              disabled={upload.isPending}
              onClick={() => folderRef.current?.click()}
            >
              <IconFolder size={14} /> Choose a folder
            </Button>
          </div>
          <p className="text-kumo-subtle text-sm">
            Up to 100 files, 4 MB each. Folders supported.
          </p>
        </div>
        {uploaded > 0 || failed > 0 || upload.isPending ? (
          <div className="mt-4" aria-label="Upload progress">
            <div className="flex justify-between font-medium">
              <span>
                Uploaded {uploaded}/{files.length}
              </span>
              <span className="text-kumo-subtle font-normal">
                {uploaded} succeeded, {failed} failed
              </span>
            </div>
            <div className="bg-kumo-recessed mt-2 h-1.5 overflow-hidden rounded-full">
              <div
                className="bg-kumo-brand h-full"
                style={{
                  width: `${files.length ? (uploaded / files.length) * 100 : 0}%`,
                }}
              />
            </div>
          </div>
        ) : null}
        {files.length > 0 ? (
          <div
            className="mt-4 max-h-40 space-y-2 overflow-auto"
            aria-label="Selected files"
          >
            {files.map((file, index) => (
              <div
                key={`${file.name}-${index}`}
                className="ring-kumo-line flex items-center gap-2 rounded-lg px-2 py-2 ring"
              >
                <span className="bg-kumo-recessed text-kumo-subtle rounded px-1.5 py-1 text-xs">
                  {file.name.split(".").at(-1)?.toUpperCase() ?? "FILE"}
                </span>
                <span
                  className="min-w-0 truncate font-medium"
                  title={file.name}
                >
                  {file.name.split("/").at(-1)}
                </span>
                <span className="text-kumo-subtle shrink-0">
                  {file.size < 1024
                    ? `${file.size} B`
                    : `${(file.size / 1024).toFixed(1)} KB`}
                </span>
                <span className="ml-auto shrink-0">
                  {statuses[index] === "success" ? (
                    <IconCircleCheck
                      size={16}
                      className="text-kumo-success"
                      aria-label="Uploaded"
                    />
                  ) : statuses[index] === "failed" ? (
                    <span className="text-kumo-danger">Failed</span>
                  ) : (
                    <Button
                      shape="square"
                      variant="ghost"
                      aria-label={`Remove ${file.name}`}
                      disabled={upload.isPending}
                      onClick={() => removeFile(index)}
                    >
                      <IconX size={16} />
                    </Button>
                  )}
                </span>
              </div>
            ))}
          </div>
        ) : null}
        {error ? (
          <p className="text-kumo-danger mt-3 text-sm" role="alert">
            {error}
          </p>
        ) : null}
        <div className="border-kumo-line mt-4 flex items-center justify-between border-t pt-3">
          <Button variant="ghost" disabled={upload.isPending} onClick={close}>
            Close
          </Button>
          <div className="flex items-center gap-2">
            {uploaded > 0 ? (
              <Button
                variant="ghost"
                disabled={upload.isPending}
                onClick={clearCompleted}
              >
                Clear completed
              </Button>
            ) : null}
            <Button
              variant="primary"
              disabled={
                files.length === 0 ||
                uploaded === files.length ||
                upload.isPending
              }
              onClick={() => upload.mutate()}
            >
              {upload.isPending ? "Uploading…" : "Upload files"}
            </Button>
          </div>
        </div>
      </Dialog>
    </Dialog.Root>
  );
}
