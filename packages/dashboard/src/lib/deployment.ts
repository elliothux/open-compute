export function buildWorkerDeploymentMetadata(input: {
  mainModule: string;
  promote?: boolean;
}): string {
  return JSON.stringify({
    mainModule: input.mainModule,
    vars: {},
    secrets: {},
    bindings: {},
    services: {},
    promote: input.promote ?? false,
  }).replace(/[^\x20-\x7e]/g, value => `\\u${value.charCodeAt(0).toString(16).padStart(4, "0")}`);
}
