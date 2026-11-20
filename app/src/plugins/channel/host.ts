// ── 宿主侧信道（iframe 引擎，2026-10-18）──────────────────────────
//
// 一个 iframe 一条信道：握手 → mount / execute → 请求-响应 + 事件。
// 所有消息都过 `protocol.ts` 的信封校验、来源校验与能力校验。

import {
  API_VERSION,
  CHANNEL_VERSION,
  ChannelError,
  checkMethod,
  makeEnvelope,
  makeError,
  negotiateChannelVersion,
  parseEnvelope,
  type ChannelEnvelope,
  type ChannelErrorCode,
  type ChannelMethod,
} from "./protocol";

type Handler = (payload: unknown) => unknown | Promise<unknown>;

/**
 * iframe 文档的地址（`ui/index.html`）。
 *
 * **不能一律写 `lunac-plugin://`**：Windows / Android 的 WebView **无法直接加载自定义 scheme**
 * —— Tauri 在这两类平台上把它映射成 `http://<scheme>.localhost/`（与
 * `convertFileSrc` / `http://tauri.localhost` 同一条机制）。写 `lunac-plugin://` 时
 * WebView2 按未知 scheme 处理：导航失败、`load` 不触发、`error` 也不触发，
 * 表现就是「3s 未握手：文档没有触发 load」（2026-10-29 探针真机首次暴露）。
 */
export function pluginFrameUrl(pluginId: string): string {
  const id = encodeURIComponent(pluginId);
  const windowsLike = /Windows|Android/i.test(navigator.userAgent);
  return windowsLike
    ? `http://lunac-plugin.localhost/${id}/ui/index.html`
    : `lunac-plugin://localhost/${id}/ui/index.html`;
}

export interface ChannelHostOptions {
  iframe: HTMLIFrameElement;
  pluginId: string;
  permissions: readonly string[];
  engine?: "dom" | "iframe";
  /** iframe 的预期 origin（Windows 自定义协议 = `http://lunac-plugin.localhost`）。 */
  origin: string;
  /** 额外认得的 origin（一般不用给：`http://` / `https://` 两个映射会自动互相补上）。 */
  origins?: readonly string[];
  /** `custom` 只发明暗模式，不发宿主 token。 */
  appearance?: "inherit" | "custom";
  getTheme?: () => { mode: string; tokens?: Record<string, string> };
  locale?: string;
  handlers?: Partial<Record<ChannelMethod, Handler>>;
  onResize?: (size: { height: number; width?: number }) => void;
  onLog?: (level: string, message: string) => void;
  onReady?: (channelVersion: number) => void;
  onError?: (error: ChannelError) => void;
  requestTimeoutMs?: number;
  handshakeTimeoutMs?: number;
}

interface Pending {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

const CLAMP_MIN = 100;
const CLAMP_MAX = 4000;

export class ChannelHost {
  private readonly options: ChannelHostOptions;
  private ready = false;
  private disposed = false;
  private version = CHANNEL_VERSION;
  private seq = 0;
  private pending = new Map<string, Pending>();
  private handshakeTimer: ReturnType<typeof setTimeout> | null = null;
  /**
   * 认得的 iframe origin。**两个映射都收**：Windows 上 `lunac-plugin://localhost/x` 由 WebView2
   * 映射成 `http://lunac-plugin.localhost/x`，但某些配置（HTTPS scheme / 未来 Tauri 版本）是
   * `https://` 那份。只认一个的话，映射一变就是**静默失败**：`postMessage` 的 targetOrigin
   * 对不上时浏览器直接丢弃，既不报错也不触发任何事件，最后只剩「3s 未握手」。
   */
  private readonly allowedOrigins: string[];
  /** 握手期间**来源对不上**的消息（诊断用；只记自己 iframe 发来的，别窗口的不记）。 */
  private rejectedOrigins: string[] = [];
  private iframeLoaded = false;

  constructor(options: ChannelHostOptions) {
    this.options = options;
    const alt = options.origin.startsWith("http://")
      ? `https://${options.origin.slice("http://".length)}`
      : options.origin.startsWith("https://")
        ? `http://${options.origin.slice("https://".length)}`
        : "";
    this.allowedOrigins = Array.from(
      new Set([options.origin, alt, ...(options.origins ?? [])].filter(Boolean)),
    );
  }

  start(): void {
    window.addEventListener("message", this.onMessage);
    this.options.iframe.addEventListener("load", this.onLoad);
    this.options.iframe.addEventListener("error", this.onFrameError);
    this.handshakeTimer = setTimeout(() => {
      this.fail(new ChannelError("E_TIMEOUT", this.handshakeFailureHint()));
    }, this.options.handshakeTimeoutMs ?? 3000);
  }

  /** 握手失败时**说清卡在哪一步**（否则只剩一句「3s 未握手」，排查全凭猜）。 */
  private handshakeFailureHint(): string {
    if (!this.iframeLoaded) {
      return "iframe 未在 3s 内完成握手：文档没有触发 load（协议未注册 / 路径 404 / `lunac-plugin://` 在 Windows 上不可直接加载；地址必须由 `pluginFrameUrl()` 生成）";
    }
    if (this.rejectedOrigins.length > 0) {
      const list = Array.from(new Set(this.rejectedOrigins)).join("、");
      return `iframe 未在 3s 内完成握手：收到了消息但来源不被接受（实际 origin=${list}，期望 ${this.allowedOrigins.join(" / ")}）`;
    }
    return "iframe 未在 3s 内完成握手：文档已加载但一条消息都没收到（脚本未执行 / 被 CSP 拦 / 没有回 ready）";
  }

  async mount(input: unknown, slot = "window", size?: { width: number; height: number }): Promise<unknown> {
    return this.request("mount", { input, slot, size }, 10_000);
  }

  async execute(input: unknown): Promise<unknown> {
    return this.request("execute", { input }, 10_000);
  }

  themeChanged(): void {
    if (!this.ready) return;
    this.sendTheme();
  }

  localeChanged(locale: string): void {
    if (!this.ready) return;
    this.sendEvent("locale.changed", { locale });
  }

  /** 宿主视口变化（设计 C.5 的 `host.resize`）：常驻锚点在折叠 / 展开、设置分页切换时
   *  用它让插件重新排版（挂载时的初始尺寸可能取到 0）。 */
  resizeTo(size: { width: number; height: number }): void {
    if (!this.ready) return;
    this.sendEvent("host.resize", size);
  }

  dispose(): void {
    if (this.disposed) return;
    if (this.ready) {
      this.sendEvent("lifecycle", { phase: "dispose" });
    }
    this.disposed = true;
    if (this.handshakeTimer) clearTimeout(this.handshakeTimer);
    this.handshakeTimer = null;
    window.removeEventListener("message", this.onMessage);
    this.options.iframe.removeEventListener("load", this.onLoad);
    this.options.iframe.removeEventListener("error", this.onFrameError);
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer);
      pending.reject(new ChannelError("E_CANCELLED", `请求 ${id} 已取消`));
    }
    this.pending.clear();
  }

  private onLoad = (): void => {
    if (this.disposed) return;
    this.iframeLoaded = true;
    const theme = this.options.getTheme?.() ?? { mode: "dark" };
    this.sendEvent("hello", {
      channelVersions: [CHANNEL_VERSION],
      apiVersion: API_VERSION,
      pluginId: this.options.pluginId,
      caps: [...this.options.permissions],
      themeMode: theme.mode,
      locale: this.options.locale ?? navigator.language,
    });
    // 设计顺序：hello → theme.changed → mount。先给快照，避免首帧白闪。
    this.sendTheme();
  };

  /** iframe 的 `error`（文档压根没取到）—— 直接失败，别让用户干等 3s。 */
  private onFrameError = (): void => {
    if (this.disposed) return;
    this.fail(new ChannelError("E_HOST", "iframe 加载失败（lunac-plugin:// 未返回文档）"));
  };

  private sendTheme(): void {
    const theme = this.options.getTheme?.() ?? { mode: "dark" };
    if (this.options.appearance === "custom") {
      this.sendEvent("theme.changed", { mode: theme.mode });
    } else {
      this.sendEvent("theme.changed", { mode: theme.mode, tokens: theme.tokens ?? {} });
    }
  }

  private onMessage = (event: MessageEvent): void => {
    if (this.disposed) return;
    const expected = this.options.iframe.contentWindow;
    if (!expected || event.source !== expected) return; // 别的窗口的消息：忽略、**不记**（避免噪声）
    if (!event.origin || event.origin === "*" || !this.allowedOrigins.includes(event.origin)) {
      // 来源不对：**记一笔**，握手失败时能说出「实际 origin 是什么」
      if (this.rejectedOrigins.length < 4) this.rejectedOrigins.push(event.origin || "(空)");
      return;
    }
    const envelope = parseEnvelope(event.data);
    if (!envelope) return;
    if (envelope.v !== this.version) {
      this.sendError(envelope.id, envelope.method, new ChannelError("E_VERSION", `不支持信道版本 ${envelope.v}`));
      return;
    }
    if (envelope.type === "res" || envelope.type === "err") {
      this.settle(envelope);
      return;
    }
    if (envelope.type === "event") {
      this.handleEvent(envelope);
      return;
    }
    if (envelope.type === "req") {
      void this.handleRequest(envelope);
    }
  };

  private settle(envelope: ChannelEnvelope): void {
    const pending = this.pending.get(envelope.id);
    if (!pending) return;
    clearTimeout(pending.timer);
    this.pending.delete(envelope.id);
    if (envelope.type === "res") {
      pending.resolve(envelope.payload);
      return;
    }
    const payload = envelope.payload as { code?: string; message?: string } | undefined;
    pending.reject(new ChannelError((payload?.code as ChannelErrorCode) ?? "E_HOST", payload?.message ?? "插件返回错误"));
  }

  private handleEvent(envelope: ChannelEnvelope): void {
    const payload = (envelope.payload ?? {}) as Record<string, unknown>;
    switch (envelope.method) {
      case "ready": {
        const version = Number(payload.channelVersion);
        if (!Number.isInteger(version) || version !== this.version) {
          this.fail(new ChannelError("E_VERSION", `插件选择了不支持的信道版本 ${String(payload.channelVersion)}`));
          return;
        }
        this.ready = true;
        if (this.handshakeTimer) clearTimeout(this.handshakeTimer);
        this.handshakeTimer = null;
        this.options.onReady?.(version);
        return;
      }
      case "ui.resize": {
        const height = clamp(Number(payload.height));
        const width = payload.width === undefined ? undefined : clamp(Number(payload.width));
        if (height !== null) this.options.onResize?.({ height, width: width ?? undefined });
        return;
      }
      case "log": {
        this.options.onLog?.(String(payload.level ?? "info"), String(payload.message ?? ""));
        return;
      }
      case "bye":
        this.dispose();
        return;
      default:
        return;
    }
  }

  private async handleRequest(envelope: ChannelEnvelope): Promise<void> {
    const error = checkMethod(envelope.method, this.options.permissions, this.options.engine ?? "iframe");
    if (error) {
      this.sendError(envelope.id, envelope.method, error);
      return;
    }
    const handler = this.options.handlers?.[envelope.method as ChannelMethod];
    if (!handler) {
      this.sendError(envelope.id, envelope.method, new ChannelError("E_UNSUPPORTED", `宿主未实现 ${envelope.method}`));
      return;
    }
    try {
      const result = await handler(envelope.payload);
      this.send("res", envelope.method, result, envelope.id);
    } catch (err) {
      const error = err instanceof ChannelError
        ? err
        : new ChannelError("E_HOST", err instanceof Error ? err.message : String(err));
      this.sendError(envelope.id, envelope.method, error);
    }
  }

  private request(method: string, payload: unknown, timeoutMs: number): Promise<unknown> {
    if (!this.ready) {
      return Promise.reject(new ChannelError("E_NOT_READY", "信道尚未握手"));
    }
    const id = this.nextId();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new ChannelError("E_TIMEOUT", `${method} 超时（${timeoutMs}ms）`));
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
      this.send("req", method, payload, id);
    });
  }

  private sendEvent(method: string, payload: unknown): void {
    this.send("event", method, payload, this.nextId());
  }

  private sendError(id: string, method: string, error: ChannelError): void {
    this.sendEnvelope(makeError(id, method, error));
  }

  private send(type: ChannelEnvelope["type"], method: string, payload: unknown, id: string): void {
    this.sendEnvelope(makeEnvelope(type, method, id, payload));
  }

  private sendEnvelope(envelope: ChannelEnvelope): void {
    const target = this.options.iframe.contentWindow;
    if (!target) return;
    target.postMessage(envelope, this.options.origin);
  }

  private nextId(): string {
    this.seq += 1;
    return `h${Date.now().toString(36)}-${this.seq}`;
  }

  private fail(error: ChannelError): void {
    if (this.handshakeTimer) clearTimeout(this.handshakeTimer);
    this.handshakeTimer = null;
    this.options.onError?.(error);
  }
}

function clamp(value: number): number | null {
  if (!Number.isFinite(value)) return null;
  return Math.min(CLAMP_MAX, Math.max(CLAMP_MIN, Math.round(value)));
}
