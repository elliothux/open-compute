const RETRYABLE =
  /(?:R2ProviderUnavailable|R2_PROVIDER_UNAVAILABLE|R2 operation timed out|operation timed out|temporarily busy|operation queue is saturated|overloaded|resource limit|subrequest limit|isolate|dispatch)/i;

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function isRetryableR2Error(error: unknown): boolean {
  if (!(error instanceof Error)) {
    return RETRYABLE.test(String(error));
  }
  return RETRYABLE.test(error.message) || RETRYABLE.test(error.name);
}

/** Retry transient R2 contention under peak read load. */
export async function withR2Retry<T>(
  operation: () => Promise<T>,
  maxAttempts = 8,
): Promise<T> {
  let lastError: unknown;
  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    try {
      return await operation();
    } catch (error) {
      lastError = error;
      if (!isRetryableR2Error(error) || attempt === maxAttempts) {
        throw error;
      }
      await delay(15 * attempt);
    }
  }
  throw lastError;
}
