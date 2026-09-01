import handler from "vinext/server/fetch-handler";

interface Env {
  readonly ASSETS: Fetcher;
}

export default {
  fetch(request: Request, env: Env, context: ExecutionContext): Promise<Response> {
    return handler.fetch(request, env, context);
  },
};
