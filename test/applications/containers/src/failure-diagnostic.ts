export function fixtureErrorCode(error: unknown): string | null {
  if (error instanceof DOMException) {
    if (error.name === "TimeoutError") return "REQUEST_TIMEOUT";
    if (error.name === "AbortError") return "REQUEST_ABORTED";
  }
  const message = error instanceof Error ? error.message : "";
  const nativeMessages = [
    ["Container operation failed.", "CONTAINER_OPERATION_FAILED"],
    ["Network connection lost.", "NETWORK_CONNECTION_LOST"],
    ["The Container is not running.", "CONTAINER_NOT_RUNNING"],
    ["Container execution authority changed.", "CONTAINER_AUTHORITY_CHANGED"],
    ["Container port connection closed unexpectedly.", "CONTAINER_PORT_CLOSED"],
    ["Container port connection read aborted.", "CONTAINER_PORT_READ_ABORTED"],
  ] as const;
  return (
    [
      "CONTAINER_UNAVAILABLE",
      "DO_RUNTIME_EXCEPTION",
      "DO_VERSION_STALE",
      "DO_OBJECT_DELETING",
      "DO_STORAGE_UNAVAILABLE",
      "VERSION_INVARIANT_VIOLATION",
      "DO_ID_INVALID",
      "DO_NAMESPACE_NOT_FOUND",
      "DO_INTERNAL_PROTOCOL_ERROR",
      "SERVICE_BINDING_DENIED",
      "DO_RPC_UNSUPPORTED",
    ].find((code) => message === code) ??
    nativeMessages.find(([text]) => message === text)?.[1] ??
    null
  );
}

const LOOPBACK_PROBE = [
  "import http.client,json",
  "try: state=open('/proc/1/stat').read(4096).rsplit(')',1)[1].split()[0]",
  "except Exception: state='?'",
  "counts={'0A':0,'01':0,'08':0}",
  "try:",
  " for name in ('tcp','tcp6'):",
  "  for line in open('/proc/net/'+name).read(1<<20).splitlines()[1:]:",
  "   fields=line.split()",
  "   if fields[1].split(':')[1]=='1F90' and fields[3] in counts: counts[fields[3]]+=1",
  "except Exception: counts={key:-1 for key in counts}",
  "status=None; error=None; size=None",
  "connection=http.client.HTTPConnection('127.0.0.1',8080,timeout=1)",
  "try:",
  " connection.request('GET','/',headers={'Connection':'close'})",
  " response=connection.getresponse(); status=response.status; size=len(response.read(4096))",
  "except Exception as failure:",
  " name=type(failure).__name__",
  " error=name if name in ('TimeoutError','ConnectionRefusedError','ConnectionResetError','RemoteDisconnected','HTTPException','OSError') else 'Other'",
  "finally: connection.close()",
  "print(json.dumps({'pidState':state,'listening':counts['0A'],'established':counts['01'],'closeWait':counts['08'],'httpStatus':status,'httpError':error,'httpBytes':size}),end='')",
].join("\n");

async function loopbackDiagnostic(
  native: NonNullable<DurableObjectState["container"]>,
) {
  try {
    const process = await native.exec(["python3", "-c", LOOPBACK_PROBE], {
      signal: AbortSignal.timeout(3000),
      stdout: "pipe",
      stderr: "pipe",
    });
    const output = await process.output();
    const lengths = {
      exitCode: output.exitCode,
      stdoutBytes: output.stdout.byteLength,
      stderrBytes: output.stderr.byteLength,
    };
    if (
      output.exitCode !== 0 ||
      output.stdout.byteLength > 1024 ||
      output.stderr.byteLength !== 0
    )
      return lengths;
    const value: unknown = JSON.parse(new TextDecoder().decode(output.stdout));
    if (
      typeof value !== "object" ||
      value === null ||
      Array.isArray(value) ||
      Object.keys(value).sort().join(",") !==
        "closeWait,established,httpBytes,httpError,httpStatus,listening,pidState" ||
      !("pidState" in value) ||
      typeof value.pidState !== "string" ||
      !/^[RSDTtXZPIKW?]$/.test(value.pidState) ||
      !("listening" in value) ||
      !("established" in value) ||
      !("closeWait" in value) ||
      [value.listening, value.established, value.closeWait].some(
        (count) =>
          typeof count !== "number" ||
          !Number.isSafeInteger(count) ||
          count < -1 ||
          count > 65536,
      ) ||
      !("httpStatus" in value) ||
      (value.httpStatus !== null &&
        (typeof value.httpStatus !== "number" ||
          !Number.isInteger(value.httpStatus) ||
          value.httpStatus < 100 ||
          value.httpStatus > 599)) ||
      !("httpBytes" in value) ||
      (value.httpBytes !== null &&
        (typeof value.httpBytes !== "number" ||
          !Number.isInteger(value.httpBytes) ||
          value.httpBytes < 0 ||
          value.httpBytes > 4096)) ||
      !("httpError" in value) ||
      (value.httpError !== null &&
        (typeof value.httpError !== "string" ||
          ![
            "TimeoutError",
            "ConnectionRefusedError",
            "ConnectionResetError",
            "RemoteDisconnected",
            "HTTPException",
            "OSError",
            "Other",
          ].includes(value.httpError)))
    )
      return { ...lengths, outputRejected: true };
    return { ...lengths, ...value };
  } catch (error) {
    return { failure: await fixtureFailureDiagnostic(error) };
  }
}

export async function readinessDiagnostic(
  native: NonNullable<DurableObjectState["container"]>,
  attempts: number,
  lastStatus: number | null,
  error: unknown,
) {
  // Probe only after the original readiness bound failed; never turn a failure into success.
  return {
    attempts,
    lastStatus,
    nativeRunning: native.running,
    lastFailure:
      error === undefined ? null : await fixtureFailureDiagnostic(error),
    loopback: await loopbackDiagnostic(native),
  };
}

export async function fixtureFailureDiagnostic(error: unknown) {
  const message = error instanceof Error ? error.message : "";
  const bytes = new TextEncoder().encode(message.slice(0, 4096));
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  const messageSha256 = Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
  const originalStack = error instanceof Error ? (error.stack ?? "") : "";
  const firstFrame = originalStack.indexOf("\n") + 1;
  const stack = originalStack.slice(firstFrame, firstFrame + 8192);
  const sites = [
    "createStubPolicy",
    "StubPolicy.fetch",
    "NamespacePolicy.get",
    "makeId",
    "dispatchFetch",
    "routeFetch",
    "DoHost",
    "FixtureContainer",
    "NamedImageContainer",
  ].filter((site) => stack.includes(site));
  return {
    nativeErrorCode: fixtureErrorCode(error),
    nativeErrorMessageBytes: bytes.byteLength,
    nativeErrorMessageTruncated: message.length > 4096,
    nativeErrorMessageSha256: messageSha256,
    nativeErrorSites: sites,
    nativeErrorInternalReference: message.startsWith("internal error"),
    nativeErrorClass:
      error instanceof TypeError
        ? "TypeError"
        : error instanceof Error
          ? "Error"
          : "Unknown",
  };
}
