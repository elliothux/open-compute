const RETRYABLE =
  /(?:D1_?(?:OVERLOADED|TIMEOUT|BUSY)|SQLITE_BUSY|database is locked|database is busy|temporarily busy|operation queue is saturated)/i;

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function isRetryableD1Error(error: unknown): boolean {
  if (!(error instanceof Error)) {
    return RETRYABLE.test(String(error));
  }
  return RETRYABLE.test(error.message) || RETRYABLE.test(error.name);
}

/** Retry transient D1 contention under peak write/read load. */
export async function withD1Retry<T>(
  operation: () => Promise<T>,
  maxAttempts = 8,
): Promise<T> {
  let lastError: unknown;
  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    try {
      return await operation();
    } catch (error) {
      lastError = error;
      if (!isRetryableD1Error(error) || attempt === maxAttempts) {
        throw error;
      }
      await delay(15 * attempt);
    }
  }
  throw lastError;
}
