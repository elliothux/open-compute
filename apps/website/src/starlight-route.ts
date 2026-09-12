import { defineRouteMiddleware } from "@astrojs/starlight/route-data";
import { createDocsSidebar, flattenDocsSidebar } from "./docs-navigation";

export const onRequest = defineRouteMiddleware(({ locals, url }) => {
  const isChinese =
    url.pathname === "/docs/zh/" || url.pathname.startsWith("/docs/zh/");
  const language = isChinese ? "zh" : "en";
  const route = locals.starlightRoute;
  const sidebar = createDocsSidebar(language, url.pathname);
  const flatSidebar = flattenDocsSidebar(sidebar);
  const currentIndex = flatSidebar.findIndex((entry) => entry.isCurrent);

  route.lang = isChinese ? "zh-CN" : "en";
  route.entryMeta.lang = route.lang;
  route.siteTitleHref = "/";
  route.sidebar = sidebar;
  route.pagination = {
    prev: currentIndex > 0 ? flatSidebar[currentIndex - 1] : undefined,
    next:
      currentIndex >= 0 && currentIndex < flatSidebar.length - 1
        ? flatSidebar[currentIndex + 1]
        : undefined,
  };
});
