import { readFile } from "node:fs/promises";
import type { Browser } from "@cloudflare/playwright";

/** Exercise the fixed client's own Worker filesystem, without adding a client shim. */
export async function downloads(
  browser: Browser,
  accept: boolean | undefined,
): Promise<Record<string, unknown>> {
  const context = await browser.newContext(
    accept === undefined ? {} : { acceptDownloads: accept },
  );
  try {
    const page = await context.newPage();
    await page.setContent(
      '<a download="fixture.txt" href="data:text/plain;base64,Ynl0ZS1kZWxpdmVyeQ==">download</a>',
    );
    const pending = page.waitForEvent("download", { timeout: 5000 });
    await page.click("a");
    const download = await pending;
    const result: Record<string, unknown> = {
      filename: download.suggestedFilename(),
      failure: await download.failure(),
    };
    if (result.failure === null) {
      const path = await download.path();
      result.virtualPath = path.startsWith("/tmp/playwright-artifacts-");
      try {
        result.pathBytes = (await readFile(path)).toString();
      } catch (error) {
        result.pathError = error instanceof Error ? error.message : "unknown";
      }
      try {
        await download.saveAs("/tmp/browser-download-saved.txt");
        result.savedBytes = (
          await readFile("/tmp/browser-download-saved.txt")
        ).toString();
      } catch (error) {
        result.saveError = error instanceof Error ? error.message : "unknown";
      }
      try {
        const stream = await download.createReadStream();
        const chunks: Buffer[] = [];
        for await (const chunk of stream)
          chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
        result.streamBytes = Buffer.concat(chunks).toString();
      } catch (error) {
        result.streamError = error instanceof Error ? error.message : "unknown";
      }
    }
    return result;
  } finally {
    await context.close();
  }
}
