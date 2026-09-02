import { APIError } from "cloudflare/error";
import { useToast } from "./ToastProvider";

export function useMutationFeedback() {
  const { pushToast } = useToast();

  return {
    success(message: string) {
      pushToast(message, "success");
    },
    failure(error: unknown, fallback: string) {
      pushToast(
        typeof error === "string"
          ? error
          : error instanceof APIError
            ? error.message
            : fallback,
        "error",
      );
    },
  };
}
