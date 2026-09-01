function failure(code: string): Error {
  const error = Object.assign(new Error(code), { stableCode: code });
  error.stack = `Error: ${code}`;
  return error;
}

/** Typed CONNECT authority carried only across open-compute's private tunnel hops. */
export type SocketAuthorityWire =
  | Readonly<{ kind: "string"; address: string }>
  | Readonly<{ kind: "record"; hostname: string; port: number }>;

const CONTROL = /[\x00-\x1f\x7f]/;

function authorityText(authority: SocketAuthorityWire): string {
  return authority.kind === "string"
    ? authority.address
    : `${authority.hostname}:${authority.port}`;
}

/** Preserve whether the public caller supplied a string or a SocketAddress record. */
export function socketAuthorityWire(address: SocketAddress | string): SocketAuthorityWire {
  return typeof address === "string"
    ? Object.freeze({ kind: "string", address })
    : Object.freeze({ kind: "record", hostname: address.hostname, port: address.port });
}

/** Validate and clone an untrusted private-protocol authority value. */
export function validateSocketAuthorityWire(value: unknown): SocketAuthorityWire {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw failure("SOCKET_TUNNEL_INVALID");
  }
  const item = value as Record<string, unknown>;
  if (item.kind === "string" && Object.keys(item).length === 2
      && typeof item.address === "string" && item.address.length >= 3
      && item.address.length <= 1_024 && !CONTROL.test(item.address)) {
    return Object.freeze({ kind: "string", address: item.address });
  }
  if (item.kind === "record" && Object.keys(item).length === 3
      && typeof item.hostname === "string" && item.hostname.length > 0
      && !CONTROL.test(item.hostname) && Number.isInteger(item.port)
      && Number(item.port) >= 0 && Number(item.port) <= 65_535) {
    const authority = Object.freeze({
      kind: "record" as const,
      hostname: item.hostname,
      port: Number(item.port),
    });
    const text = authorityText(authority);
    if (text.length >= 3 && text.length <= 1_024) return authority;
  }
  throw failure("SOCKET_TUNNEL_INVALID");
}

/** Recover the native address only when the inbound CONNECT authority matches its wire value. */
export function socketAddressFromWire(
  value: unknown,
  observedAuthority?: string,
): SocketAddress | string {
  const authority = validateSocketAuthorityWire(value);
  if (observedAuthority !== undefined && authorityText(authority) !== observedAuthority) {
    throw failure("SOCKET_TUNNEL_INVALID");
  }
  return authority.kind === "string"
    ? authority.address
    : { hostname: authority.hostname, port: authority.port };
}

/** Read the CONNECT authority supplied to an inbound Worker connect handler. */
export async function inboundSocketAddress(socket: Socket): Promise<string> {
  const info = await socket.opened;
  if (typeof info.localAddress !== "string" || info.localAddress.length < 3
      || info.localAddress.length > 1024 || /[\x00-\x1f\x7f]/.test(info.localAddress)) {
    throw failure("SOCKET_TUNNEL_INVALID");
  }
  return info.localAddress;
}

/** Recover a native connect target from workerd's inbound authority text. */
export async function inboundSocketTargetAddress(socket: Socket): Promise<SocketAddress | string> {
  const address = await inboundSocketAddress(socket);
  const separator = address.lastIndexOf(":");
  if (!address.startsWith("[") && separator > 0 && address.indexOf(":") !== separator) {
    const portText = address.slice(separator + 1);
    const port = Number(portText);
    if (/^[0-9]{1,5}$/.test(portText) && Number.isInteger(port) && port <= 65_535) {
      return { hostname: address.slice(0, separator), port };
    }
  }
  return address;
}

/** Drain a native Socket tunnel while preserving independent half-close in both directions. */
export async function tunnelSockets(left: Socket, right: Socket): Promise<void> {
  const directions = Promise.allSettled([
    left.readable.pipeTo(right.writable),
    right.readable.pipeTo(left.writable),
  ]);
  const disconnected = Promise.race([
    left.closed.then(() => undefined, () => undefined),
    right.closed.then(() => undefined, () => undefined),
  ]);
  const outcome = await Promise.race([
    directions.then(results => ({ results })),
    disconnected.then(() => ({ results: undefined })),
  ]);
  if (!outcome.results) {
    await Promise.allSettled([left.close(), right.close()]);
  }
  const results = outcome.results ?? await directions;
  const failed = results.some(result => result.status === "rejected");
  if (failed) {
    await Promise.allSettled([left.close(), right.close()]);
    throw failure("SOCKET_TUNNEL_FAILED");
  }
}
