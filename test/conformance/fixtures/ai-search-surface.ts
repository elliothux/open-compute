interface AiSearchSurfaceEnv {
  SEARCH: AiSearchNamespace;
  DIRECT_SEARCH: AiSearchInstance;
}

export default {
  async fetch(_request: Request, env: AiSearchSurfaceEnv): Promise<Response> {
    const created = await env.SEARCH.create({
      id: "docs",
      index_method: { vector: true, keyword: true },
    });
    const listed = await env.SEARCH.list({ page: 1, per_page: 10, order_by: "created_at" });
    const selected = env.SEARCH.get("docs");
    const uploaded = await created.items.upload("guide.txt", "search guide");
    const completed = await created.items.uploadAndPoll("guide.md", "# Search guide");
    const items = await created.items.list({ page: 1, per_page: 10 });
    const item = created.items.get(uploaded.id);
    const itemInfo = await item.info();
    const content = await item.download();
    const synced = await item.sync();
    const itemLogs = await item.logs({ limit: 10 });
    const chunks = await item.chunks({ limit: 10, offset: 0 });
    const search = await selected.search({ query: "search guide" });
    const info = await selected.info();
    const stats = await selected.stats();
    const updated = await selected.update({ cache: false });
    const jobCreated = await selected.jobs.create({});
    const jobs = await selected.jobs.list({ page: 1, per_page: 10 });
    const job = selected.jobs.get(jobCreated.id);
    const jobInfo = await job.info();
    const jobLogs = await job.logs({ page: 1, per_page: 10 });
    const cancelled = await job.cancel();
    const multi = await env.SEARCH.search({
      query: "search guide",
      ai_search_options: { instance_ids: ["docs"] },
    });
    const chat = await env.DIRECT_SEARCH.chatCompletions({
      messages: [{ role: "user", content: "summarize" }],
    });
    const chatStream = await env.DIRECT_SEARCH.chatCompletions({
      messages: [{ role: "user", content: "summarize" }],
      stream: true,
    });
    const multiChat = await env.SEARCH.chatCompletions({
      messages: [{ role: "user", content: "summarize" }],
      ai_search_options: { instance_ids: ["docs"] },
    });
    const multiChatStream = await env.SEARCH.chatCompletions({
      messages: [{ role: "user", content: "summarize" }],
      stream: true,
      ai_search_options: { instance_ids: ["docs"] },
    });
    await created.items.delete(completed.id);
    await env.SEARCH.delete("docs");
    return Response.json({
      listed,
      items,
      itemInfo,
      contentType: content.contentType,
      synced,
      itemLogs,
      chunks,
      search,
      info,
      stats,
      updated,
      jobs,
      jobInfo,
      jobLogs,
      cancelled,
      multi,
      chat,
      chatStream: chatStream instanceof ReadableStream,
      multiChat,
      multiChatStream: multiChatStream instanceof ReadableStream,
    });
  },
} satisfies ExportedHandler<AiSearchSurfaceEnv>;
