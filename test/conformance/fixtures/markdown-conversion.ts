import type { OpenComputeAi } from "open-compute:ai";

interface MarkdownConversionEnv {
  AI: OpenComputeAi;
}

export default {
  async fetch(_request: Request, env: MarkdownConversionEnv): Promise<Response> {
    const document: MarkdownDocument = {
      name: "manual.pdf",
      blob: new Blob(["%PDF fixture"], { type: "application/pdf" }),
    };
    const direct = await env.AI.toMarkdown(document, {
      conversionOptions: { output: { format: "markdown" }, pdf: { metadata: true } },
    });
    const directBatch = await env.AI.toMarkdown([document]);
    const converter = env.AI.toMarkdown();
    const handled = await converter.transform(document, {
      conversionOptions: { output: { format: "text" } },
    });
    const handledBatch = await converter.transform([document]);
    const supported = await converter.supported();
    const aiGatewayLogId: string | null = env.AI.aiGatewayLogId;

    return Response.json({
      aiGatewayLogId,
      direct: direct.format,
      directBatch: directBatch.length,
      handled: handled.format,
      handledBatch: handledBatch.length,
      supported: supported.length,
    });
  },
} satisfies ExportedHandler<MarkdownConversionEnv>;
