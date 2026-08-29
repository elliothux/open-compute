interface AssetRequestWire {
  readonly url: string;
  readonly method: string;
  readonly headers: readonly (readonly [string, string])[];
}

interface AssetTransport {
  fetchAsset(request: AssetRequestWire): Promise<Response>;
}

/** Tenant-visible Fetcher facade backed by one deployment-scoped trusted transport. */
export class AssetsBinding {
  readonly #transport: AssetTransport;

  constructor(transport: unknown) {
    if (!transport || typeof transport !== "object"
        || typeof (transport as Partial<AssetTransport>).fetchAsset !== "function") {
      throw new TypeError("ASSET_BINDING_UNAVAILABLE");
    }
    this.#transport = transport as AssetTransport;
  }

  fetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
    const request = new Request(input, init);
    return this.#transport.fetchAsset({
      url: request.url,
      method: request.method,
      headers: [...request.headers],
    });
  }
}
