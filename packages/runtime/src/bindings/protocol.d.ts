/** Backend capability available only to platform-owned transport entrypoints. */
export interface BindingEnv {
  BINDING_BACKEND: Fetcher;
  BINDING_BACKEND_TOKEN: string;
}

/** Resource authority selected from the persisted deployment descriptor. */
export interface BindingProps {
  bindingId: string;
  deploymentId: string;
  descriptorSha256: string;
}
export interface AssetBindingProps {
  deploymentId: string;
  descriptorSha256: string;
}
export interface CacheTransportProps {
  accountId: string;
  workerId: string;
  deploymentId: string;
  entrypoint: string;
  descriptorSha256: string;
  automaticEnabled: boolean;
  crossVersionCache: boolean;
}
export interface ImageTransportProps {
  accountId: string;
  workerId: string;
  deploymentId: string;
  descriptorSha256: string;
}
export interface AiTransportProps {
  accountId: string;
  workerId: string;
  deploymentId: string;
  descriptorSha256: string;
}
export interface ServiceBindingProps {
  deploymentId: string;
  bindingName: string;
  descriptorSha256: string;
  entrypoint?: string;
}
export interface ResourceBindingProps extends BindingProps {
  accountId: string;
  workerId: string;
  routeGeneration: number;
  namespaceResourceId: string;
  resourceSpecGeneration: number;
  permissions: { read: boolean; write: boolean };
}
/** Constructs a sanitized product error from a stable code. */
export type BindingError = (code: string) => Error;
