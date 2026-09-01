interface Env {
  VERSION: WorkerVersionMetadata;
}

async function cacheSurface(request: Request, ctx: ExecutionContext): Promise<void> {
  const named = await caches.open("portable");
  await named.put(request, new Response("cached"));
  await named.match(request, { ignoreMethod: true });
  await named.delete(request, { ignoreMethod: false });
  if (ctx.cache === undefined) throw new Error("cache context unavailable");
  const purge: CachePurgeResult = await ctx.cache.purge({
    tags: ["portable"],
    pathPrefixes: ["/portable"],
    purgeEverything: false,
  });
  const success: boolean = purge.success;
  const errors: CachePurgeError[] = purge.errors;
  for (const error of errors) {
    const code: number = error.code;
    const message: string = error.message;
    void code;
    void message;
  }
  void success;
}

const scheduled: ExportedHandlerScheduledHandler<Env> = async (controller, env, ctx) => {
  const cron: string = controller.cron;
  const scheduledTime: number = controller.scheduledTime;
  controller.noRetry();
  const id: string = env.VERSION.id;
  const tag: string = env.VERSION.tag;
  const timestamp: string = env.VERSION.timestamp;
  ctx.waitUntil(cacheSurface(new Request("https://portable.invalid/cache"), ctx));
  void cron;
  void scheduledTime;
  void id;
  void tag;
  void timestamp;
};

function serviceWorkerScheduled(event: ScheduledEvent): void {
  const cron: string = event.cron;
  const scheduledTime: number = event.scheduledTime;
  event.noRetry();
  event.waitUntil(Promise.resolve());
  void cron;
  void scheduledTime;
}

export default { scheduled } satisfies ExportedHandler<Env>;
void serviceWorkerScheduled;
