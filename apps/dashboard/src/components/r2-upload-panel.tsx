import { Button } from "@cloudflare/kumo/components/button";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import {
  IconFile as FileIcon,
  IconCircleCheck,
  IconCloudUpload,
  IconX,
} from "@tabler/icons-react";
import { useEffect, useRef, useState } from "react";
import { useAuth } from "../features/auth/auth-atoms";
import { formatBytes } from "../lib/format";

const PART_SIZE = 10 * 1024 * 1024;
const MULTIPART_THRESHOLD = 100 * 1024 * 1024;
const DASHBOARD_UPLOAD_LIMIT = 300 * 1024 * 1024;

type UploadEntry = {
  id: string;
  key: string;
  file: File;
  progress: number;
  status: "queued" | "uploading" | "done" | "failed";
  error?: string;
};

export function R2UploadPanel({
  bucketId,
  prefix,
  initialFiles,
  onInitialFilesHandled,
  onClose,
  onUploaded,
}: {
  bucketId: string;
  prefix: string;
  initialFiles: File[];
  onInitialFilesHandled: () => void;
  onClose: () => void;
  onUploaded: () => Promise<void>;
}) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const filesInput = useRef<HTMLInputElement>(null);
  const folderInput = useRef<HTMLInputElement>(null);
  const active = useRef(false);
  const lastInitialFiles = useRef<File[] | null>(null);
  const [entries, setEntries] = useState<UploadEntry[]>([]);
  const [uploading, setUploading] = useState(false);
  const done = entries.filter((entry) => entry.status === "done").length;
  const failed = entries.filter((entry) => entry.status === "failed").length;

  const update = (id: string, patch: Partial<UploadEntry>) =>
    setEntries((current) =>
      current.map((entry) =>
        entry.id === id ? { ...entry, ...patch } : entry,
      ),
    );

  const uploadOne = async (entry: UploadEntry) => {
    if (!client || !selectedInstanceId)
      throw new Error("The account is unavailable.");
    if (entry.file.size > DASHBOARD_UPLOAD_LIMIT) {
      throw new Error("Files over 300 MB require the S3 API or a Worker.");
    }
    if (entry.file.size < MULTIPART_THRESHOLD) {
      await client.r2.buckets.objects.upload(
        entry.key,
        entry.file,
        { account_id: selectedInstanceId, bucket_name: bucketId },
        {
          headers: {
            "Content-Type": entry.file.type || "application/octet-stream",
          },
        },
      );
      return;
    }
    const created = await client.openCompute.r2.multipart.create(
      selectedInstanceId,
      bucketId,
      {
        key: entry.key,
        options: {
          httpMetadata: {
            contentType: entry.file.type || "application/octet-stream",
          },
        },
      },
    );
    try {
      const parts = [];
      const total = Math.ceil(entry.file.size / PART_SIZE);
      for (let index = 0; index < total; index += 1) {
        const part = await client.openCompute.r2.multipart.uploadPart(
          selectedInstanceId,
          bucketId,
          created.uploadId,
          String(index + 1),
          entry.key,
          entry.file.slice(
            index * PART_SIZE,
            Math.min(entry.file.size, (index + 1) * PART_SIZE),
          ),
        );
        parts.push(part);
        update(entry.id, { progress: Math.round(((index + 1) / total) * 100) });
      }
      await client.openCompute.r2.multipart.complete(
        selectedInstanceId,
        bucketId,
        created.uploadId,
        entry.key,
        { parts },
      );
    } catch (error) {
      await client.openCompute.r2.multipart
        .abort(selectedInstanceId, bucketId, created.uploadId, entry.key)
        .catch(() => undefined);
      throw error;
    }
  };

  const uploadFiles = async (files: File[]) => {
    if (!files.length || active.current) return;
    const batch = files.map((file) => ({
      id: crypto.randomUUID(),
      key: `${prefix}${file.webkitRelativePath || file.name}`,
      file,
      progress: 0,
      status: "queued" as const,
    }));
    active.current = true;
    setUploading(true);
    setEntries((current) => [...current, ...batch]);
    let changed = false;
    try {
      for (const entry of batch) {
        update(entry.id, { status: "uploading" });
        try {
          await uploadOne(entry);
          update(entry.id, { status: "done", progress: 100 });
          changed = true;
        } catch (error) {
          update(entry.id, {
            status: "failed",
            error: error instanceof Error ? error.message : "Upload failed.",
          });
        }
      }
      if (changed) await onUploaded();
    } finally {
      active.current = false;
      setUploading(false);
    }
  };

  useEffect(() => {
    if (!initialFiles.length || initialFiles === lastInitialFiles.current)
      return;
    lastInitialFiles.current = initialFiles;
    onInitialFilesHandled();
    void uploadFiles(initialFiles);
  });

  return (
    <LayerCard className="min-w-0 px-5 py-4">
      <input
        ref={filesInput}
        type="file"
        multiple
        className="hidden"
        aria-label="Choose files to upload"
        onChange={(event) => {
          void uploadFiles(Array.from(event.target.files ?? []));
          event.target.value = "";
        }}
      />
      <input
        ref={folderInput}
        type="file"
        multiple
        {...{ webkitdirectory: "" }}
        className="hidden"
        aria-label="Choose folder to upload"
        onChange={(event) => {
          void uploadFiles(Array.from(event.target.files ?? []));
          event.target.value = "";
        }}
      />
      {entries.length ? (
        <div className="grid gap-4">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2 font-medium">
              <IconCircleCheck size={18} className="text-kumo-success" />
              {done}/{entries.length} files uploaded
            </div>
            <div className="flex gap-2">
              <Button
                variant="secondary"
                disabled={uploading}
                onClick={() => setEntries([])}
              >
                Clear all
              </Button>
              <Button
                variant="secondary"
                disabled={uploading}
                onClick={onClose}
              >
                Close
              </Button>
            </div>
          </div>
          <div
            className={
              failed
                ? "bg-kumo-danger/10 rounded-md px-4 py-3"
                : "bg-kumo-tint rounded-md px-4 py-3"
            }
          >
            {uploading
              ? "Uploading files…"
              : failed
                ? `${failed} file${failed === 1 ? "" : "s"} failed to upload.`
                : "All files uploaded successfully."}
          </div>
          <div className="divide-kumo-line divide-y">
            {entries.map((entry) => (
              <div
                key={entry.id}
                className="flex min-w-0 items-start gap-3 py-3 text-sm"
              >
                <span className="flex h-lh items-center">
                  <FileIcon size={16} />
                </span>
                <div className="min-w-0 flex-1">
                  <p className="break-all">{entry.key.slice(prefix.length)}</p>
                  {entry.error ? (
                    <p className="text-kumo-danger">{entry.error}</p>
                  ) : null}
                </div>
                <span className="text-kumo-subtle shrink-0">
                  {formatBytes(entry.file.size)}
                </span>
                <span
                  className="shrink-0"
                  aria-label={`${entry.key}: ${entry.status}`}
                >
                  {entry.status === "done" ? (
                    <IconCircleCheck size={16} className="text-kumo-success" />
                  ) : entry.status === "uploading" ? (
                    `${entry.progress}%`
                  ) : entry.status === "failed" ? (
                    "Failed"
                  ) : (
                    "Queued"
                  )}
                </span>
              </div>
            ))}
          </div>
        </div>
      ) : (
        <div className="grid gap-3">
          <div className="flex justify-end">
            <Button
              variant="secondary"
              shape="square"
              aria-label="Close upload"
              onClick={onClose}
            >
              <IconX size={16} />
            </Button>
          </div>
          <div
            className="border-kumo-line flex min-h-60 flex-col items-center justify-center gap-6 rounded-md border border-dashed px-5 py-8 md:flex-row"
            onDragOver={(event) => event.preventDefault()}
            onDrop={(event) => {
              event.preventDefault();
              void uploadFiles(Array.from(event.dataTransfer.files));
            }}
          >
            <IconCloudUpload
              size={108}
              strokeWidth={1.5}
              className="text-kumo-subtle shrink-0"
            />
            <div className="grid gap-2 text-center md:text-left">
              <p className="font-medium">
                Your bucket is ready. Add files to get started.
              </p>
              <div className="flex flex-wrap items-center justify-center gap-2 md:justify-start">
                <span className="text-kumo-subtle">Drag and drop or</span>
                <Button
                  variant="secondary"
                  onClick={() => filesInput.current?.click()}
                >
                  Choose files
                </Button>
                <Button
                  variant="secondary"
                  onClick={() => folderInput.current?.click()}
                >
                  Choose a folder
                </Button>
              </div>
              <p className="text-kumo-subtle text-xs">
                Files over 300 MB require the S3-compatible API or Workers.
              </p>
            </div>
          </div>
        </div>
      )}
    </LayerCard>
  );
}
