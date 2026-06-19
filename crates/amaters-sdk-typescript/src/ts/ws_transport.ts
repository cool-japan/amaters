/**
 * WebSocket transport for AmateRS SDK.
 *
 * Provides a browser-compatible alternative to gRPC-web when the native
 * HTTP/1.1 transport is unavailable. Supports automatic reconnection and
 * periodic heartbeats so long-lived connections stay alive through proxies.
 *
 * @example
 * ```typescript
 * import { WebSocketTransport } from '@amaters/sdk';
 *
 * const transport = new WebSocketTransport({
 *   url: 'wss://my-server.example.com/amaters-ws',
 * });
 * transport.onMessage((data) => { console.log('received', data.byteLength, 'bytes'); });
 * await transport.connect();
 * await transport.send(new Uint8Array([1, 2, 3]));
 * ```
 */

/** Configuration for {@link WebSocketTransport}. */
export interface WsTransportConfig {
  /** WebSocket endpoint URL (`ws://` or `wss://`). */
  url: string;
  /**
   * Milliseconds to wait before attempting a reconnect after a disconnect.
   * @default 1000
   */
  reconnectDelayMs?: number;
  /**
   * Maximum number of reconnection attempts before giving up.
   * @default 5
   */
  maxReconnectAttempts?: number;
  /**
   * Interval in milliseconds between heartbeat pings sent over the socket.
   * Set to 0 to disable heartbeats.
   * @default 30000
   */
  heartbeatIntervalMs?: number;
}

/** Callback invoked for each inbound binary message. */
export type MessageHandler = (data: Uint8Array) => void;

/** Callback invoked when an unrecoverable transport error occurs. */
export type ErrorHandler = (error: Error) => void;

/**
 * WebSocket transport layer for AmateRS.
 *
 * Wraps the browser (or Node.js 22+) `WebSocket` API with reconnection
 * logic and heartbeat support. Binary frames are used exclusively —
 * string frames are ignored.
 *
 * The transport is intentionally *not* async-iterable; instead callers
 * register handlers via {@link onMessage} and {@link onError}. This keeps
 * the API simple and avoids backpressure complexity for the transport layer.
 */
export class WebSocketTransport {
  private readonly config: Required<WsTransportConfig>;

  private ws: WebSocket | null = null;
  private reconnectAttempts = 0;
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null;
  private _isConnected = false;

  private messageHandlers: MessageHandler[] = [];
  private errorHandlers: ErrorHandler[] = [];

  constructor(config: WsTransportConfig) {
    this.config = {
      reconnectDelayMs: config.reconnectDelayMs ?? 1000,
      maxReconnectAttempts: config.maxReconnectAttempts ?? 5,
      heartbeatIntervalMs: config.heartbeatIntervalMs ?? 30_000,
      url: config.url,
    };
  }

  /** `true` when the underlying WebSocket is in the `OPEN` state. */
  get isConnected(): boolean {
    return this._isConnected;
  }

  /**
   * Establish the WebSocket connection.
   *
   * Resolves once the socket reaches the `OPEN` state. Rejects if the
   * socket cannot be opened at all (no reconnect is attempted for the
   * initial connection attempt).
   */
  async connect(): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      try {
        this.ws = new WebSocket(this.config.url);
        this.ws.binaryType = 'arraybuffer';
      } catch (err) {
        reject(err instanceof Error ? err : new Error(String(err)));
        return;
      }

      const ws = this.ws;

      ws.onopen = (): void => {
        this._isConnected = true;
        this.reconnectAttempts = 0;
        this.startHeartbeat();
        resolve();
      };

      ws.onmessage = (event: MessageEvent): void => {
        if (event.data instanceof ArrayBuffer) {
          const data = new Uint8Array(event.data);
          for (const handler of this.messageHandlers) {
            handler(data);
          }
        }
        // String frames (e.g. heartbeat acks) are intentionally ignored.
      };

      ws.onerror = (): void => {
        // onerror is always followed by onclose; we handle recovery there.
        // Only reject the connect() promise if we were never connected.
        if (!this._isConnected) {
          reject(new Error(`WebSocket error connecting to ${this.config.url}`));
        }
      };

      ws.onclose = (): void => {
        this._isConnected = false;
        this.stopHeartbeat();
        this.scheduleReconnect();
      };
    });
  }

  /**
   * Send a binary message.
   *
   * @throws {Error} If the transport is not currently connected.
   */
  async send(data: Uint8Array): Promise<void> {
    if (this.ws === null || !this._isConnected) {
      throw new Error('WebSocketTransport: not connected');
    }
    this.ws.send(data);
  }

  /**
   * Register a handler for inbound binary messages.
   *
   * Multiple handlers may be registered; each is called in registration order.
   */
  onMessage(handler: MessageHandler): void {
    this.messageHandlers.push(handler);
  }

  /**
   * Register a handler for transport errors.
   *
   * Errors are reported when reconnection is exhausted or a fatal error
   * occurs that cannot be recovered from.
   */
  onError(handler: ErrorHandler): void {
    this.errorHandlers.push(handler);
  }

  /**
   * Close the WebSocket connection and stop reconnection attempts.
   *
   * After calling `close()`, no further reconnection attempts are made.
   */
  close(): void {
    this.reconnectAttempts = this.config.maxReconnectAttempts; // exhaust reconnect budget
    this.stopHeartbeat();
    if (this.ws !== null) {
      this.ws.onclose = null; // prevent scheduleReconnect from firing
      this.ws.close();
      this.ws = null;
    }
    this._isConnected = false;
  }

  // ---------------------------------------------------------------------------
  // Private helpers
  // ---------------------------------------------------------------------------

  private startHeartbeat(): void {
    if (this.config.heartbeatIntervalMs <= 0) return;
    this.stopHeartbeat();
    this.heartbeatTimer = setInterval(() => {
      if (this._isConnected && this.ws !== null) {
        try {
          // Send a minimal ping frame (1-byte sentinel).
          this.ws.send(new Uint8Array([0x00]));
        } catch {
          // Swallow: the onclose handler will trigger reconnection.
        }
      }
    }, this.config.heartbeatIntervalMs);
  }

  private stopHeartbeat(): void {
    if (this.heartbeatTimer !== null) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  private scheduleReconnect(): void {
    if (this.reconnectAttempts >= this.config.maxReconnectAttempts) {
      const err = new Error(
        `WebSocketTransport: max reconnect attempts (${this.config.maxReconnectAttempts}) reached for ${this.config.url}`,
      );
      for (const handler of this.errorHandlers) {
        handler(err);
      }
      return;
    }
    this.reconnectAttempts += 1;
    setTimeout(() => {
      void this.connect().catch(() => {
        // connect() rejects only on the initial open failure; after that
        // onerror/onclose will trigger scheduleReconnect again.
      });
    }, this.config.reconnectDelayMs);
  }
}
