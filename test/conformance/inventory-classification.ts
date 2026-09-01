/** Explicit product assignment for every named declaration in the pinned stable AST. */

export const PUBLIC_PRODUCTS = [
  "workers",
  "deployments",
  "static_assets",
  "service_bindings",
  "kv",
  "r2",
  "d1",
  "durable_objects",
  "alarms",
  "queues",
  "cron",
  "workflows",
  "workers_cache",
  "cache_api",
  "images",
  "version_metadata",
  "websocket_hibernation",
  "analytics_engine",
  "ai",
  "browser_rendering",
  "vectorize",
  "hyperdrive",
  "mtls",
  "rate_limiting",
  "workers_for_platforms",
] as const;

export type PublicProduct = (typeof PUBLIC_PRODUCTS)[number];

export type ProductClass = "target" | "platform" | "non_target";

export interface Classification {
  product: string;
  class: ProductClass;
}

export interface ClassificationRule {
  product: string;
  class: ProductClass;
  exact?: readonly string[];
  prefixes?: readonly string[];
}

export const PLATFORM_PRODUCTS: Record<string, { status: "supported" | "supported_with_deviation"; deviations: readonly string[] }> = {
  deployments: { status: "supported_with_deviation", deviations: ["OC-DEPLOY-001"] },
  images: { status: "supported_with_deviation", deviations: ["OC-IMAGES-001"] },
  static_assets: { status: "supported_with_deviation", deviations: ["OC-ASSETS-001"] },
  service_bindings: { status: "supported_with_deviation", deviations: ["OC-SERVICE-001"] },
  workers_cache: { status: "supported_with_deviation", deviations: ["OC-CACHE-001", "OC-CACHE-002"] },
};

export const TARGET_PRODUCT_DEVIATIONS: Record<string, readonly string[]> = {
  workers: ["OC-WKR-LIMIT-001", "OC-WKR-TCP-001"],
  kv: ["OC-KV-001"],
  r2: ["OC-R2-001"],
  d1: ["OC-D1-001"],
  durable_objects: ["OC-DO-001"],
  queues: ["OC-QUEUE-001"],
  workflows: ["OC-WORKFLOW-001"],
  cron: ["OC-CRON-001"],
  cache_api: ["OC-CACHE-001", "OC-CACHE-002"],
};

export const NON_TARGET_PUBLIC_PRODUCTS = [
  "analytics_engine",
  "ai",
  "browser_rendering",
  "vectorize",
  "hyperdrive",
  "mtls",
  "rate_limiting",
  "workers_for_platforms",
] as const;

/** First match wins. Non-target Cloudflare products are listed before workers remainder. */
export const CLASSIFICATION_RULES: readonly ClassificationRule[] = [
  { product: "ai", class: "non_target", prefixes: [
    "Ai", "BaseAi", "Base_Ai", "AutoRAG", "AutoRag", "AIGateway", "AiGateway",
    "ChatCompletion", "ChatCompletions", "ChatTemplate", "ResponseInput", "ResponseOutput", "ResponseFunction",
    "ResponseFormat", "ResponseStream", "ResponseText", "ResponseUsage",
    "ResponseStatus", "ResponseItem", "ResponseError", "ResponseCreated",
    "ResponseCompleted", "ResponseFailed", "ResponseIncomplete", "ResponseRefusal",
    "ResponseReasoning", "ResponseCustom", "ResponseConversation", "ResponsePrompt",
    "ResponseIncludable", "ResponseContent", "EasyInputMessage", "FunctionDefinition",
    "FunctionMessage", "DeveloperMessage", "SystemMessage", "UserMessage",
    "AssistantMessage", "ToolMessage", "ToolChoice", "UsageTags", "RoleScoped",
    "PromptTokens", "CompletionTokens", "CompletionUsage", "AudioParams",
    "PredictionContent", "ReasoningEffort", "InferenceUpstream", "GatewayRetries",
    "GatewayOptions", "UniversalGateway", "Artifacts", "AgentMemory", "Flagship",
    "WebSearch",
  ], exact: [
    "Tool", "Without", "XOR", "Reasoning", "Logprob", "TopLogprob",
    "ResponsesInput", "ResponsesOutput", "ResponsesFunctionTool",
    "ComparisonFilter", "CompoundFilter", "StreamOptions",
  ] },
  { product: "vectorize", class: "non_target", prefixes: ["Vectorize", "VectorFloatArray"] },
  { product: "hyperdrive", class: "non_target", prefixes: ["Hyperdrive"] },
  { product: "analytics_engine", class: "non_target", prefixes: ["AnalyticsEngine"] },
  { product: "browser_rendering", class: "non_target", prefixes: ["Browser"] },
  { product: "rate_limiting", class: "non_target", prefixes: ["RateLimit"] },
  { product: "workers_for_platforms", class: "non_target", prefixes: [
    "WorkerLoader", "WorkerStub", "workerdResourceLimits", "ColoLocalActorNamespace",
    "LoopbackColoLocalActorNamespace", "DispatchNamespace", "DynamicDispatch",
    "ExportedHandlerTestHandler",
  ] },
  { product: "mtls", class: "non_target", prefixes: ["IncomingRequestCfPropertiesTLS"], exact: ["FetcherMtls"] },
  { product: "email", class: "non_target", prefixes: [
    "Email", "ForwardableEmail", "SendEmail", "cloudflare:email",
  ] },
  { product: "pipelines", class: "non_target", prefixes: ["cloudflare:pipelines", "Pipeline"] },
  { product: "tail", class: "non_target", prefixes: [
    "TailEvent", "TailStream", "TraceEvent", "TraceLog", "ScriptVersion",
    "TraceItem", "TracePreview", "TraceException", "TraceDiagnostic",
    "TraceMetrics", "UnsafeTraceMetrics", "ExportedHandlerTail", "ExportedHandlerTrace",
  ] },
  { product: "pages", class: "non_target", prefixes: ["assets:"], exact: [
    "PagesFunction", "PagesPluginFunction", "EventContext", "EventPluginContext", "Params",
  ] },
  { product: "stream", class: "non_target", prefixes: [
    "StreamBinding", "StreamVideo", "StreamPublic", "StreamDirect", "StreamUrl",
    "StreamScoped", "StreamWatermark", "StreamCaption", "StreamDownload",
    "StreamWatermarks", "StreamPagination", "StreamError", "StreamUpdate", "StreamVideos",
  ], exact: [
    "InternalError", "BadRequestError", "NotFoundError", "ForbiddenError",
    "QuotaReachedError", "MaxFileSizeError", "InvalidURLError", "AlreadyUploadedError",
    "TooManyWatermarksError", "RateLimitedError",
  ] },
  { product: "media", class: "non_target", prefixes: ["MediaBinding", "MediaTransformer", "MediaTransformation", "MediaError"] },
  { product: "hosted_images", class: "non_target", prefixes: [
    "HostedImages", "ImageHandle", "ImageList", "ImageUpload", "ImageUpdate",
    "ImageSigned", "ImageDirect", "ImageMetadataFilter",
  ] },
  { product: "secrets_store", class: "non_target", prefixes: ["SecretsStore"] },
  { product: "pubsub", class: "non_target", prefixes: ["PubSub"] },
  { product: "containers", class: "non_target", prefixes: [
    "Container", "ExecOutput", "ExecProcess",
  ] },
  { product: "access", class: "non_target", prefixes: ["CloudflareAccess"] },
  { product: "hello_world", class: "non_target", prefixes: ["HelloWorld"] },
  { product: "markdown", class: "non_target", prefixes: [
    "MarkdownDocument", "ToMarkdown", "ConversionResponse", "ConversionOutput",
    "ConversionOptions", "ConversionRequest", "SupportedFileFormat",
  ], exact: ["OutputFormat"] },
  { product: "kv", class: "target", prefixes: ["KVNamespace"] },
  { product: "r2", class: "target", prefixes: ["R2"] },
  { product: "d1", class: "target", prefixes: ["D1"] },
  { product: "durable_objects", class: "target", prefixes: [
    "DurableObject", "SqlStorage", "SyncKv", "LoopbackDurableObject",
    "Rpc.DurableObject", "CloudflareWorkersModule.DurableObject",
  ], exact: ["AlarmInvocationInfo", "DurableObjectFacets", "FacetStartupOptions"] },
  { product: "queues", class: "target", prefixes: ["Queue"], exact: [
    "Message", "MessageBatch", "MessageBatchMetrics", "MessageBatchMetadata",
    "MessageSendRequest", "ExportedHandlerQueueHandler", "QueueContentType",
  ] },
  { product: "workflows", class: "target", prefixes: [
    "Workflow", "CloudflareWorkersModule.Workflow", "Rpc.Workflow", "cloudflare:workflows",
  ], exact: ["InstanceStatus"] },
  { product: "cache_api", class: "target", prefixes: ["Cache"] },
  { product: "images", class: "non_target", prefixes: [
    "ImagesBinding", "ImagesError", "ImageTransformer", "ImageTransformation",
    "ImageInfoResponse", "ImageSource", "ImageTransform", "ImageDraw",
    "ImageInputOptions", "ImageOutputOptions", "ImageMetadata", "ImageConversion",
    "EmbeddedImageConversion", "BasicImageTransformations", "RequestInitCfPropertiesImage",
    "TextRasterize", "TextOptions",
  ] },
  { product: "version_metadata", class: "target", exact: ["WorkerVersionMetadata"] },
  { product: "cron", class: "target", exact: [
    "ScheduledEvent", "ScheduledController", "ExportedHandlerScheduledHandler",
  ] },
  { product: "websocket_hibernation", class: "target", exact: ["WebSocketRequestResponsePair"] },
];

export const MEMBER_PRODUCT_OVERRIDES: ReadonlyMap<string, string> = new Map([
  ["ExecutionContext.access", "workers_for_platforms"],
  ["ExportedHandler.email", "workers_for_platforms"],
  ["ExportedHandler.tail", "workers_for_platforms"],
  ["ExportedHandler.tailStream", "workers_for_platforms"],
  ["ExportedHandler.test", "workers_for_platforms"],
  ["ExportedHandler.trace", "workers_for_platforms"],
  ["CloudflareWorkersModule.WorkerEntrypoint.email", "workers_for_platforms"],
  ["CloudflareWorkersModule.WorkerEntrypoint.tail", "workers_for_platforms"],
  ["CloudflareWorkersModule.WorkerEntrypoint.tailStream", "workers_for_platforms"],
  ["CloudflareWorkersModule.WorkerEntrypoint.test", "workers_for_platforms"],
  ["CloudflareWorkersModule.WorkerEntrypoint.trace", "workers_for_platforms"],
  ["ServiceWorkerGlobalScope.TailEvent", "workers_for_platforms"],
  ["ServiceWorkerGlobalScope.TraceEvent", "workers_for_platforms"],
  ["DurableObjectStorage.getAlarm", "alarms"],
  ["DurableObjectStorage.setAlarm", "alarms"],
  ["DurableObjectStorage.deleteAlarm", "alarms"],
  ["DurableObjectTransaction.getAlarm", "alarms"],
  ["DurableObjectTransaction.setAlarm", "alarms"],
  ["DurableObjectTransaction.deleteAlarm", "alarms"],
  ["DurableObject.alarm", "alarms"],
  ["DurableObjectState.acceptWebSocket", "websocket_hibernation"],
  ["DurableObjectState.getWebSockets", "websocket_hibernation"],
  ["DurableObjectState.setWebSocketAutoResponse", "websocket_hibernation"],
  ["DurableObjectState.getWebSocketAutoResponse", "websocket_hibernation"],
  ["DurableObjectState.getWebSocketAutoResponseTimestamp", "websocket_hibernation"],
  ["DurableObjectState.setHibernatableWebSocketEventTimeout", "websocket_hibernation"],
  ["DurableObjectState.getHibernatableWebSocketEventTimeout", "websocket_hibernation"],
  ["DurableObjectState.getTags", "websocket_hibernation"],
  ["DurableObject.webSocketMessage", "websocket_hibernation"],
  ["DurableObject.webSocketClose", "websocket_hibernation"],
  ["DurableObject.webSocketError", "websocket_hibernation"],
  ["CloudflareWorkersModule.DurableObject.webSocketMessage", "websocket_hibernation"],
  ["CloudflareWorkersModule.DurableObject.webSocketClose", "websocket_hibernation"],
  ["CloudflareWorkersModule.DurableObject.webSocketError", "websocket_hibernation"],
  ["WebSocket.serializeAttachment", "websocket_hibernation"],
  ["WebSocket.deserializeAttachment", "websocket_hibernation"],
]);

const EXACT = new Map<string, Classification>();
const PREFIXES: { prefix: string; classification: Classification }[] = [];

for (const rule of CLASSIFICATION_RULES) {
  const classification = { product: rule.product, class: rule.class };
  for (const name of rule.exact ?? []) {
    if (EXACT.has(name)) throw new Error(`duplicate exact classification: ${name}`);
    EXACT.set(name, classification);
  }
  for (const prefix of rule.prefixes ?? []) {
    PREFIXES.push({ prefix, classification });
  }
}

PREFIXES.sort((left, right) => right.prefix.length - left.prefix.length || left.prefix.localeCompare(right.prefix));

export function classifySymbol(name: string): Classification {
  const exact = EXACT.get(name);
  if (exact !== undefined) return exact;
  for (const rule of PREFIXES) {
    if (name === rule.prefix) return rule.classification;
    if (!name.startsWith(rule.prefix)) continue;
    const prefixEnd = rule.prefix.charAt(rule.prefix.length - 1);
    const next = name.charAt(rule.prefix.length);
    if (!/[A-Za-z0-9]/.test(prefixEnd) || /[A-Z0-9.:*_\-]/.test(next)) {
      return rule.classification;
    }
  }
  return { product: "workers", class: "target" };
}

export function memberProduct(symbol: string, member: string, fallback: string): string {
  return MEMBER_PRODUCT_OVERRIDES.get(`${symbol}.${member}`) ?? fallback;
}
