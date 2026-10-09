const RETRYABLE =
  /(?:WORKFLOW_RUNTIME_UNAVAILABLE|WORKFLOW_VERSION_NOT_READY|WORKFLOW_BINDING_STALE|temporarily busy|operation queue is saturated|overloaded|resource limit|subrequest limit|isolate|dispatch|not ready)/i;

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function isRetryableWorkflowError(error: unknown): boolean {
  if (!(error instanceof Error)) {
    return RETRYABLE.test(String(error));
  }
  return RETRYABLE.test(error.message) || RETRYABLE.test(error.name);
}

/** Retry transient workflow admission failures under peak create load. */
export async function withWorkflowRetry<T>(
  operation: () => Promise<T>,
  maxAttempts = 6,
): Promise<T> {
  let lastError: unknown;
  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    try {
      return await operation();
    } catch (error) {
      lastError = error;
      if (!isRetryableWorkflowError(error) || attempt === maxAttempts) {
        throw error;
      }
      await delay(20 * attempt);
    }
  }
  throw lastError;
}
