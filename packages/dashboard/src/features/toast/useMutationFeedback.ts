import { OperatorApiError } from "@open-compute/operator-sdk";
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
          : error instanceof OperatorApiError
            ? error.message
            : fallback,
        "error",
      );
    },
  };
}
