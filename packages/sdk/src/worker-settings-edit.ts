import type { APIPromise, Cloudflare } from "cloudflare";
import type { BaseCloudflare } from "cloudflare/client";
import type { ScriptAndVersionSettingEditResponse } from "cloudflare/resources/workers/scripts/script-and-version-settings";

/** Send the OpenAPI-declared JSON multipart part without losing empty arrays. */
export function editWorkerSettings(
  transport: BaseCloudflare,
  scriptName: string,
  params: { account_id: string; settings?: unknown },
  options?: Cloudflare.RequestOptions,
): APIPromise<ScriptAndVersionSettingEditResponse> {
  const path = [params.account_id, scriptName].map((value) => {
    if (!value || value === "." || value === "..")
      throw new Error("invalid Worker path segment");
    return encodeURIComponent(value);
  });
  const form = new FormData();
  form.append(
    "settings",
    new Blob([JSON.stringify(params.settings ?? {})], {
      type: "application/json",
    }),
    "settings.json",
  );
  return transport
    .patch<{ result: ScriptAndVersionSettingEditResponse }>(
      `/accounts/${path[0]}/workers/scripts/${path[1]}/settings`,
      { ...options, body: form },
    )
    ._thenUnwrap((envelope) => envelope.result);
}
