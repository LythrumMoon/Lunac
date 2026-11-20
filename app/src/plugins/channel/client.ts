// ── 插件侧信道客户端（iframe 引擎，2026-10-18）────────────────────
//
// 插件作者把这份文件（或编译产物）放进 `ui/`，在 iframe 里调用
// `connectPluginChannel()`：握手、收发 mount/execute、请求宿主能力。
//
// 安全：只接受 `event.source === window.parent` 的消息；父窗口 origin
// 从第一条 `hello` 事件里学习，后续发送都用它作为 targetOrigin。

import {
  CHANNEL_VERSION,
  ChannelError,
  makeEnvelope,
  makeError,
  negotiateChannelVersion,
  parseEnvelope,
  ResizeCoalescer,
  type ChannelEnvelope,
  type ChannelErrorCode,
  type ChannelMethod,
} from "./protocol";

export interface PluginChannelHandlers {
  onMount?(payload: { input: unknown; slot: string; size?: { width: number; height: number } }): void;
  onExecute?(payload: { input: unknown }): void;
  onTheme?(payload: { mode?: string; tokens?: Record<string, string> }): void;
  onLocale?(payload: { locale?: string }): void;
  onLifecycle?(payload: { phase?: string }): void;
  onError?(error: ChannelError): void;
}

export interface PluginChannelClient {
  /** 握手应答；必须在收到 `hello` 后、`onMount` 之前调用（客户端会自动调用一次）。 */
  ready(capabilitiesUsed?: string[]): void;
  resize(height: number, width?: number): void;
  status(text: string): void;
  request<T = unknown>(method: ChannelMethod, payload?: unknown, timeoutMs?: number): Promise<T>;
  dispose(): void;
}

/** 把宿主下发的 token 写到 iframe 自己的 documentElement；`inherit` 插件直接 `var(--...)`。 */
export function applyThemeTokens(tokens: Record<string, string> | undefined): void {
  if (!tokens) return;
  const style = document.documentElement.style;
  for (const [name, value] of Object.entries(tokens)) {
    if (name.startsWith("--") && value) style.setProperty(name, value);
  }
}

export function connectPluginChannel(handlers: PluginChannelHandlers = {}): PluginChannelClient {
  let version: number | null = null;
  let parentOrigin: string | null = null;
  let seq = 0;
  let disposed = false;
  let autoReady = false;
  const pending = new Map<string, {
    resolve: (value: unknown) => void;
    reject: (error: Error) => void;
    timer: ReturnType<typeof setTimeout>;
  }>();
  const resizer = new ResizeCoalescer(32, (size) => {
    send("event", "ui.resize", size, nextId());
  });

  function nextId(): string {
    seq += 1;
    return `p${Date.now().toString(36)}-${seq}`;
  }

  function send(type: ChannelEnvelope["type"], method: string, payload: unknown, id: string): void {
    if (disposed) return;
    const envelope = makeEnvelope(type, method, id, payload);
    window.parent.postMessage(envelope, parentOrigin ?? "*");
  }

  function sendError(id: string, method: string, error: ChannelError): void {
    window.parent.postMessage(makeError(id, method, error), parentOrigin ?? "*");
  }

  function ready(capabilitiesUsed: string[] = []): void {
    if (!version) return;
    send("event", "ready", { channelVersion: version, capabilitiesUsed }, nextId());
  }

  function settle(envelope: ChannelEnvelope): void {
    const item = pending.get(envelope.id);
    if (!item) return;
    clearTimeout(item.timer);
    pending.delete(envelope.id);
    if (envelope.type === "res") {
      item.resolve(envelope.payload);
      return;
    }
    const payload = envelope.payload as { code?: string; message?: string } | undefined;
    item.reject(new ChannelError((payload?.code as ChannelErrorCode) ?? "E_HOST", payload?.message ?? "宿主返回错误"));
  }

  function onMessage(event: MessageEvent): void {
    if (disposed || event.source !== window.parent) return;
    const envelope = parseEnvelope(event.data);
    if (!envelope) return;

    if (envelope.method === "hello" && envelope.type === "event") {
      parentOrigin = event.origin;
      const payload = (envelope.payload ?? {}) as {
        channelVersions?: number[];
        apiVersion?: number;
      };
      version = negotiateChannelVersion(payload.channelVersions ?? [], [CHANNEL_VERSION]);
      if (version === null) {
        handlers.onError?.(new ChannelError("E_VERSION", "没有共同的信道版本"));
        return;
      }
      if (!autoReady) {
        autoReady = true;
        ready();
      }
      return;
    }

    if (envelope.type === "res" || envelope.type === "err") {
      settle(envelope);
      return;
    }
    if (envelope.type !== "req" && envelope.type !== "event") return;

    const payload = envelope.payload ?? {};
    if (envelope.type === "event") {
      if (envelope.method === "theme.changed") {
        const theme = payload as { mode?: string; tokens?: Record<string, string> };
        applyThemeTokens(theme.tokens);
        handlers.onTheme?.(theme);
      } else if (envelope.method === "locale.changed") {
        handlers.onLocale?.(payload as { locale?: string });
      } else if (envelope.method === "lifecycle") {
        handlers.onLifecycle?.(payload as { phase?: string });
        if ((payload as { phase?: string }).phase === "dispose") dispose();
      }
      return;
    }

    try {
      if (envelope.method === "mount") {
        handlers.onMount?.(payload as { input: unknown; slot: string; size?: { width: number; height: number } });
        send("res", "mount", null, envelope.id);
      } else if (envelope.method === "execute") {
        handlers.onExecute?.(payload as { input: unknown });
        send("res", "execute", null, envelope.id);
      } else if (envelope.method === "ping") {
        send("res", "ping", null, envelope.id);
      } else {
        sendError(envelope.id, envelope.method, new ChannelError("E_UNSUPPORTED", `插件未实现 ${envelope.method}`));
      }
    } catch (err) {
      const error = err instanceof ChannelError
        ? err
        : new ChannelError("E_HOST", err instanceof Error ? err.message : String(err));
      sendError(envelope.id, envelope.method, error);
    }
  }

  window.addEventListener("message", onMessage);

  function dispose(): void {
    if (disposed) return;
    disposed = true;
    resizer.dispose();
    window.removeEventListener("message", onMessage);
    for (const item of pending.values()) {
      clearTimeout(item.timer);
      item.reject(new ChannelError("E_CANCELLED", "信道已关闭"));
    }
    pending.clear();
  }

  return {
    ready,
    resize(height: number, width?: number): void {
      resizer.push(height, width);
    },
    status(text: string): void {
      send("event", "ui.status", { text }, nextId());
    },
    request<T = unknown>(method: ChannelMethod, payload?: unknown, timeoutMs = 30_000): Promise<T> {
      if (!version) {
        return Promise.reject(new ChannelError("E_NOT_READY", "信道尚未握手"));
      }
      const id = nextId();
      return new Promise<T>((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new ChannelError("E_TIMEOUT", `${method} 超时（${timeoutMs}ms）`));
        }, timeoutMs);
        pending.set(id, { resolve: resolve as (value: unknown) => void, reject, timer });
        send("req", method, payload, id);
      });
    },
    dispose,
  };
}
