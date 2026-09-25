/** Backend capability available only to platform-owned transport entrypoints. */
export interface BindingEnv {
  BINDING_BACKEND: Fetcher;
  BINDING_BACKEND_TOKEN: string;
}

/** Resource authority selected from the persisted version descriptor. */
export interface BindingProps {
  bindingId: string;
  versionId: string;
  descriptorSha256: string;
}
export interface AssetBindingProps {
  versionId: string;
  descriptorSha256: string;
}
export interface CacheTransportProps {
  instanceId: string;
  workerId: string;
  versionId: string;
  entrypoint: string;
  descriptorSha256: string;
  automaticEnabled: boolean;
  crossVersionCache: boolean;
}
export interface ImageTransportProps {
  instanceId: string;
  workerId: string;
  versionId: string;
  descriptorSha256: string;
}
export interface AiTransportProps {
  instanceId: string;
  workerId: string;
  versionId: string;
  descriptorSha256: string;
}
export interface ServiceBindingProps {
  versionId: string;
  bindingName: string;
  descriptorSha256: string;
  entrypoint?: string;
}
export interface ResourceBindingProps extends BindingProps {
  instanceId: string;
  workerId: string;
  namespaceResourceId: string;
  resourceSpecGeneration: number;
  permissions: { read: boolean; write: boolean };
}
/** Constructs a sanitized product error from a stable code. */
export type BindingError = (code: string) => Error;
