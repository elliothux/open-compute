import { defineRouteMiddleware } from "@astrojs/starlight/route-data";
import { homeHref, localeFromPath } from "./i18n/config";

export const onRequest = defineRouteMiddleware(({ locals, url }) => {
  const language = localeFromPath(url.pathname);
  locals.starlightRoute.siteTitleHref = homeHref(language);
});
