import type { APIPromise, Cloudflare } from "cloudflare";
import type { BaseCloudflare } from "cloudflare/client";
import type {
  BaseItems,
  ItemUploadParams,
  ItemUploadResponse,
} from "cloudflare/resources/aisearch/namespaces/instances/items";

/** Preserve a browser File's relative folder path when the official SDK strips it. */
export function uploadAiSearchItem(
  transport: BaseCloudflare,
  official: BaseItems,
  id: string,
  params: ItemUploadParams,
  options?: Cloudflare.RequestOptions,
): APIPromise<ItemUploadResponse> {
  const { file } = params.file;
  if (
    typeof File === "undefined" ||
    !(file instanceof File) ||
    !file.name.includes("/")
  )
    return official.upload(id, params, options);
  const form = new FormData();
  form.append("file", file, file.name);
  if (params.file.metadata !== undefined)
    form.append("metadata", params.file.metadata);
  if (params.file.wait_for_completion !== undefined)
    form.append("wait_for_completion", String(params.file.wait_for_completion));
  const path = [params.account_id, params.name, id].map((value) => {
    if (value.length === 0 || value === "." || value === "..")
      throw new Error("invalid AI Search path segment");
    return encodeURIComponent(value);
  });
  return transport
    .post<{ result: ItemUploadResponse }>(
      `/accounts/${path[0]}/ai-search/namespaces/${path[1]}/instances/${path[2]}/items`,
      { ...options, body: form },
    )
    ._thenUnwrap((envelope) => envelope.result);
}
