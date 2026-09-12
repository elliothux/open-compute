import { defineRouteMiddleware } from "@astrojs/starlight/route-data";
import {
  flattenDocsSidebar,
  localizeDocsSidebar,
  localizeDocsTopics,
} from "./docs-topics";

export const onRequest = defineRouteMiddleware(({ locals, url }) => {
  const isChinese =
    url.pathname === "/docs/zh/" || url.pathname.startsWith("/docs/zh/");
  const language = isChinese ? "zh" : "en";
  const route = locals.starlightRoute;

  route.lang = isChinese ? "zh-CN" : "en";
  route.entryMeta.lang = route.lang;
  route.head = route.head.map((entry) =>
    entry.tag === "meta" && entry.attrs?.property === "og:locale"
      ? { ...entry, attrs: { ...entry.attrs, content: route.lang } }
      : entry,
  );
  route.siteTitleHref = "/";

  if (!locals.starlightSidebarTopics.isPageWithTopic) return;

  const sidebar = localizeDocsSidebar(route.sidebar, language);
  const flatSidebar = flattenDocsSidebar(sidebar);
  const currentIndex = flatSidebar.findIndex((entry) => entry.isCurrent);

  route.sidebar = sidebar;
  locals.starlightSidebarTopics.topics = localizeDocsTopics(
    locals.starlightSidebarTopics.topics,
    language,
  );
  route.pagination = {
    prev: currentIndex > 0 ? flatSidebar[currentIndex - 1] : undefined,
    next:
      currentIndex >= 0 && currentIndex < flatSidebar.length - 1
        ? flatSidebar[currentIndex + 1]
        : undefined,
  };
});
