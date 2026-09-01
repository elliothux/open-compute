import { NextResponse, type NextRequest } from "next/server";

export function proxy(request: NextRequest) {
  const response = NextResponse.next();
  response.headers.set("x-p4-proxy", request.nextUrl.pathname);
  return response;
}

export const config = {
  matcher: [
    "/", "/action-result", "/navigation", "/stream",
    "/pages-qualification/:path*", "/static-qualification/:path*", "/api/:path*",
  ],
};
