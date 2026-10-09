import { DurableObject } from "cloudflare:workers";
import type { Env } from "../env";

interface InventoryState {
  count: number;
  alarmTicks: number;
  lastAlarmAt: number | null;
}

function defaultState(): InventoryState {
  return {
    count: 0,
    alarmTicks: 0,
    lastAlarmAt: null,
  };
}

export class Inventory extends DurableObject<Env> {
  private async readStateTxn(
    txn: DurableObjectTransaction,
  ): Promise<InventoryState> {
    const stored = await txn.get<InventoryState>("state");
    return stored ?? defaultState();
  }

  private async mutateState(
    mutate: (state: InventoryState) => void,
  ): Promise<InventoryState> {
    return this.ctx.storage.transaction(async (txn) => {
      const state = await this.readStateTxn(txn);
      mutate(state);
      await txn.put("state", state);
      return state;
    });
  }

  async increment(
    amount: number,
  ): Promise<{ count: number; objectId: string }> {
    const state = await this.mutateState((current) => {
      current.count += amount;
    });
    if (amount > 0) {
      const alarmAt = await this.ctx.storage.getAlarm();
      if (alarmAt === null) {
        await this.ctx.storage.setAlarm(Date.now() + 50);
      }
    }
    return { count: state.count, objectId: this.ctx.id.toString() };
  }

  async read(): Promise<InventoryState & { objectId: string }> {
    const state = await this.ctx.storage.get<InventoryState>("state");
    return { ...(state ?? defaultState()), objectId: this.ctx.id.toString() };
  }

  async reserve(delta: number): Promise<{ count: number; reserved: number }> {
    const state = await this.mutateState((current) => {
      current.count += delta;
    });
    return { count: state.count, reserved: delta };
  }

  override async alarm(): Promise<void> {
    await this.mutateState((state) => {
      state.alarmTicks += 1;
      state.lastAlarmAt = Date.now();
    });
  }

  override async fetch(request: Request): Promise<Response> {
    if (request.headers.get("Upgrade")?.toLowerCase() === "websocket") {
      const pair = new WebSocketPair();
      const client = pair[0];
      const server = pair[1];
      this.ctx.acceptWebSocket(server);
      server.send(
        JSON.stringify({ type: "ready", objectId: this.ctx.id.toString() }),
      );
      return new Response(null, { status: 101, webSocket: client });
    }
    return Response.json({ error: "method_not_allowed" }, { status: 405 });
  }

  override async webSocketMessage(
    ws: WebSocket,
    message: string | ArrayBuffer,
  ): Promise<void> {
    const text =
      typeof message === "string" ? message : new TextDecoder().decode(message);
    let payload: { action?: string; amount?: number } = {};
    try {
      payload = JSON.parse(text) as { action?: string; amount?: number };
    } catch {
      ws.send(JSON.stringify({ type: "error", message: "invalid_json" }));
      return;
    }
    if (payload.action === "increment") {
      const amount = payload.amount ?? 1;
      const result = await this.increment(amount);
      ws.send(JSON.stringify({ type: "incremented", ...result }));
      return;
    }
    if (payload.action === "read") {
      const result = await this.read();
      ws.send(JSON.stringify({ type: "state", ...result }));
      return;
    }
    ws.send(JSON.stringify({ type: "echo", message: text }));
  }

  override async webSocketClose(
    _ws: WebSocket,
    code: number,
    reason: string,
    wasClean: boolean,
  ): Promise<void> {
    void code;
    void reason;
    void wasClean;
  }
}
