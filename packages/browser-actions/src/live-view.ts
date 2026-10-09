export {};

/** Deployment-owned Browser Run viewer; the capability grants only its signed session/target. */
const root = document.querySelector("main");
if (!root) throw new Error("Viewer unavailable");
const token = new URLSearchParams(location.hash.slice(1)).get("jwt");
if (!token) throw new Error("Viewer capability is missing");
const parts = token.split(".");
const encoded = parts[1];
if (!encoded) throw new Error("Viewer capability is invalid");
const claims: unknown = JSON.parse(
  atob(encoded.replace(/-/g, "+").replace(/_/g, "/")),
);
if (
  !claims ||
  typeof claims !== "object" ||
  !("session" in claims) ||
  !("target" in claims) ||
  !("mode" in claims) ||
  !("readonly" in claims) ||
  typeof claims.session !== "string" ||
  typeof claims.target !== "string" ||
  typeof claims.mode !== "string" ||
  typeof claims.readonly !== "boolean"
)
  throw new Error("Viewer capability is invalid");
const session = claims.session;
const mode = claims.mode;
const readonly = claims.readonly;
document.title = readonly
  ? "READ ONLY - Browser Live View"
  : "Browser Live View";
let target = claims.target;
const base = new URL(`./${encodeURIComponent(session)}/`, location.href);
base.hash = "";
base.search = "";
const status = document.createElement("p");
status.textContent = "Connecting…";
status.setAttribute("role", "status");
status.setAttribute("aria-live", "polite");
root.append(status);
const toolbar = document.createElement("nav");
root.append(toolbar);
const address = document.createElement("input");
address.type = "url";
address.placeholder = "https://example.com";
address.setAttribute("aria-label", "Page address");
address.disabled = readonly;
toolbar.append(address);
const go = document.createElement("button");
go.textContent = "Go";
go.disabled = readonly;
toolbar.append(go);
const tabs = document.createElement("select");
tabs.setAttribute("aria-label", "Browser tabs");
if (mode === "full") toolbar.prepend(tabs);
const frame = document.createElement("iframe");
frame.title = "Chrome DevTools";
frame.referrerPolicy = "no-referrer";
const screen = document.createElement("img");
screen.alt = "Remote browser page";
screen.tabIndex = 0;
root.append(mode === "devtools" ? frame : screen);
let socket: WebSocket | undefined;
let next = 0;
let width = 1;
let height = 1;
const pending = new Map<
  number,
  { resolve: (value: unknown) => void; reject: () => void; timer: number }
>();
function endpoint(page?: string): URL {
  const url = new URL(
    page ? `page/${encodeURIComponent(page)}` : "targets",
    base,
  );
  url.protocol = page
    ? url.protocol === "https:"
      ? "wss:"
      : "ws:"
    : url.protocol;
  url.searchParams.set("jwt", token!);
  return url;
}
function command(method: string, params: object = {}): Promise<unknown> {
  if (socket?.readyState !== WebSocket.OPEN || pending.size >= 32)
    return Promise.reject(new Error("Viewer disconnected"));
  const id = ++next;
  return new Promise((resolve, reject) => {
    const timer = window.setTimeout(() => {
      pending.delete(id);
      reject(new Error("Command deadline elapsed"));
    }, 10000);
    pending.set(id, {
      resolve,
      reject: () => reject(new Error("Browser command unavailable")),
      timer,
    });
    socket!.send(JSON.stringify({ id, method, params }));
  });
}
function disconnect(): void {
  socket?.close();
  socket = undefined;
  for (const entry of pending.values()) {
    clearTimeout(entry.timer);
    entry.reject();
  }
  pending.clear();
}
function connect(): void {
  disconnect();
  if (mode === "devtools") {
    const url = new URL(`devtools/inspector.html`, base);
    url.searchParams.set("jwt", token!);
    const ws = endpoint(target);
    url.searchParams.set(
      ws.protocol === "wss:" ? "wss" : "ws",
      `${ws.host}${ws.pathname}${ws.search}`,
    );
    frame.src = url.href;
    status.textContent = readonly ? "Read only" : "Chrome DevTools";
    return;
  }
  const current = new WebSocket(endpoint(target));
  socket = current;
  current.addEventListener("open", () => {
    if (socket !== current) return;
    status.textContent = readonly ? "Read only" : "Connected";
    void command("Page.enable")
      .then(() =>
        command("Page.startScreencast", {
          format: "jpeg",
          quality: 80,
          maxWidth: 1920,
          maxHeight: 1080,
        }),
      )
      .catch(() => {
        status.textContent = "Browser stream unavailable";
      });
  });
  current.addEventListener("message", (event: MessageEvent<unknown>) => {
    if (socket !== current || typeof event.data !== "string") return;
    let message: unknown;
    try {
      message = JSON.parse(event.data);
    } catch {
      current.close();
      return;
    }
    if (!message || typeof message !== "object") return;
    if ("id" in message && typeof message.id === "number") {
      const entry = pending.get(message.id);
      if (!entry) return;
      pending.delete(message.id);
      clearTimeout(entry.timer);
      if ("error" in message) entry.reject();
      else entry.resolve("result" in message ? message.result : undefined);
    } else if (
      "method" in message &&
      message.method === "Page.screencastFrame" &&
      "params" in message &&
      message.params &&
      typeof message.params === "object"
    ) {
      const value = message.params;
      if (
        !("data" in value) ||
        typeof value.data !== "string" ||
        !("sessionId" in value) ||
        typeof value.sessionId !== "number"
      )
        return;
      screen.src = `data:image/jpeg;base64,${value.data}`;
      if (
        "metadata" in value &&
        value.metadata &&
        typeof value.metadata === "object"
      ) {
        const metadata = value.metadata;
        if (
          "deviceWidth" in metadata &&
          typeof metadata.deviceWidth === "number"
        )
          width = metadata.deviceWidth;
        if (
          "deviceHeight" in metadata &&
          typeof metadata.deviceHeight === "number"
        )
          height = metadata.deviceHeight;
      }
      void command("Page.screencastFrameAck", {
        sessionId: value.sessionId,
      }).catch(() => current.close());
    }
  });
  current.addEventListener("close", () => {
    if (socket === current) {
      disconnect();
      status.textContent = "Disconnected";
    }
  });
  current.addEventListener("error", () => {
    status.textContent = "Connection unavailable or expired";
  });
}
go.addEventListener("click", () => {
  void command("Page.navigate", { url: address.value }).catch(() => {
    status.textContent = "Navigation unavailable";
  });
});
address.addEventListener("keydown", (event) => {
  if (event.key === "Enter") go.click();
});
tabs.addEventListener("change", () => {
  target = tabs.value;
  connect();
});
if (!readonly) {
  function position(event: MouseEvent): { x: number; y: number } | undefined {
    const bounds = screen.getBoundingClientRect();
    if (!screen.naturalWidth || !screen.naturalHeight) return;
    const scale = Math.min(
      bounds.width / screen.naturalWidth,
      bounds.height / screen.naturalHeight,
    );
    const displayedWidth = scale * screen.naturalWidth;
    const displayedHeight = scale * screen.naturalHeight;
    const x = event.clientX - bounds.left - (bounds.width - displayedWidth) / 2;
    const y =
      event.clientY - bounds.top - (bounds.height - displayedHeight) / 2;
    if (x < 0 || y < 0 || x > displayedWidth || y > displayedHeight) return;
    return {
      x: (x * width) / displayedWidth,
      y: (y * height) / displayedHeight,
    };
  }
  screen.addEventListener("click", (event) => {
    const point = position(event);
    if (!point) return;
    screen.focus();
    void command("Input.dispatchMouseEvent", {
      type: "mousePressed",
      ...point,
      button: "left",
      clickCount: 1,
    })
      .then(() =>
        command("Input.dispatchMouseEvent", {
          type: "mouseReleased",
          ...point,
          button: "left",
          clickCount: 1,
        }),
      )
      .catch(() => {
        status.textContent = "Input unavailable";
      });
  });
  screen.addEventListener(
    "wheel",
    (event) => {
      event.preventDefault();
      const point = position(event);
      if (!point) return;
      const factor =
        event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? height : 1;
      void command("Input.dispatchMouseEvent", {
        type: "mouseWheel",
        ...point,
        deltaX: event.deltaX * factor,
        deltaY: event.deltaY * factor,
      }).catch(() => {
        status.textContent = "Input unavailable";
      });
    },
    { passive: false },
  );
  const keyboard = (event: KeyboardEvent) => {
    event.preventDefault();
    const modifiers =
      Number(event.altKey) +
      2 * Number(event.ctrlKey) +
      4 * Number(event.metaKey) +
      8 * Number(event.shiftKey);
    void command("Input.dispatchKeyEvent", {
      type: event.type === "keydown" ? "keyDown" : "keyUp",
      key: event.key,
      code: event.code,
      windowsVirtualKeyCode: event.keyCode,
      modifiers,
      ...(event.type === "keydown" &&
      event.key.length === 1 &&
      !event.ctrlKey &&
      !event.altKey &&
      !event.metaKey
        ? { text: event.key }
        : {}),
    }).catch(() => {
      status.textContent = "Input unavailable";
    });
  };
  screen.addEventListener("keydown", keyboard);
  screen.addEventListener("keyup", keyboard);
}
if (mode === "full") {
  void fetch(endpoint(), {
    credentials: "omit",
    cache: "no-store",
    referrerPolicy: "no-referrer",
  })
    .then(async (response) => {
      if (!response.ok) throw new Error("Targets unavailable");
      const values: unknown = await response.json();
      if (!Array.isArray(values)) throw new Error("Targets unavailable");
      for (const value of values) {
        if (
          !value ||
          typeof value !== "object" ||
          !("type" in value) ||
          value.type !== "page" ||
          !("id" in value) ||
          typeof value.id !== "string" ||
          !("title" in value) ||
          typeof value.title !== "string"
        )
          continue;
        const option = document.createElement("option");
        option.value = value.id;
        option.textContent = value.title || "Untitled page";
        option.selected = value.id === target;
        tabs.append(option);
      }
    })
    .catch(() => {
      status.textContent = "Tabs unavailable";
    });
}
window.addEventListener("pagehide", disconnect);
connect();
