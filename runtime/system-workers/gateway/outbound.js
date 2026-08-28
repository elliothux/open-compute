// Generated from runtime/src/gateway/outbound.ts by Rolldown. Do not edit.
export default { async fetch(request) {
	const url = new URL(request.url);
	if (url.protocol !== "http:" && url.protocol !== "https:") {
		throw new TypeError("OUTBOUND_DENIED");
	}
	return fetch(new Request(request, { redirect: "follow" }));
} };
