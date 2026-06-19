/**
 * Tests for {@link WebSocketTransport}.
 *
 * All tests use a mock WebSocket to avoid needing a real server.
 * The global `WebSocket` is replaced before each test and restored after.
 */

import { strict as assert } from 'node:assert';
import { describe, it, beforeEach, afterEach } from 'node:test';

import { WebSocketTransport } from '../src/ts/ws_transport';

// ---------------------------------------------------------------------------
// Mock WebSocket
// ---------------------------------------------------------------------------

/**
 * A minimal synchronous mock that satisfies the WebSocketTransport's
 * usage of the browser WebSocket API.
 */
class MockWebSocket {
  static instances: MockWebSocket[] = [];

  readyState: number = 0; // CONNECTING
  binaryType: string = 'blob';

  onopen: (() => void) | null = null;
  onmessage: ((event: { data: ArrayBuffer | string }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;

  private sentMessages: Uint8Array[] = [];

  constructor(public url: string) {
    MockWebSocket.instances.push(this);
  }

  send(data: Uint8Array | string): void {
    if (typeof data !== 'string') {
      this.sentMessages.push(data);
    }
  }

  close(): void {
    this.readyState = 3; // CLOSED
    this.onclose?.({});
  }

  getSentMessages(): Uint8Array[] {
    return [...this.sentMessages];
  }

  // Test helpers
  simulateOpen(): void {
    this.readyState = 1; // OPEN
    this.onopen?.();
  }

  simulateMessage(data: ArrayBuffer): void {
    this.onmessage?.({ data });
  }

  simulateClose(): void {
    this.readyState = 3; // CLOSED
    this.onclose?.({});
  }

  simulateError(): void {
    this.onerror?.({});
  }
}

// Capture the real WebSocket (may be undefined in older Node)
const realWebSocket = (globalThis as unknown as Record<string, unknown>)['WebSocket'];

function installMockWs(): void {
  MockWebSocket.instances = [];
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).WebSocket = MockWebSocket;
}

function uninstallMockWs(): void {
  if (realWebSocket !== undefined) {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (globalThis as any).WebSocket = realWebSocket;
  } else {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (globalThis as any).WebSocket;
  }
}

function latestMockWs(): MockWebSocket {
  const ws = MockWebSocket.instances[MockWebSocket.instances.length - 1];
  assert.ok(ws !== undefined, 'No MockWebSocket was created');
  return ws;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('WebSocketTransport', () => {
  beforeEach(() => {
    installMockWs();
  });

  afterEach(() => {
    uninstallMockWs();
  });

  it('should connect successfully with valid config', async () => {
    const transport = new WebSocketTransport({ url: 'ws://localhost:7878' });
    const connectPromise = transport.connect();
    const mock = latestMockWs();
    mock.simulateOpen();
    await connectPromise;
    assert.equal(transport.isConnected, true);
    transport.close();
  });

  it('should send messages as Uint8Array', async () => {
    const transport = new WebSocketTransport({ url: 'ws://localhost:7878', heartbeatIntervalMs: 0 });
    const connectPromise = transport.connect();
    const mock = latestMockWs();
    mock.simulateOpen();
    await connectPromise;

    const payload = new Uint8Array([10, 20, 30]);
    await transport.send(payload);

    const sent = mock.getSentMessages();
    assert.equal(sent.length, 1, 'should have sent one message');
    assert.deepEqual(sent[0], payload, 'sent bytes should match');
    transport.close();
  });

  it('should throw when sending while not connected', async () => {
    const transport = new WebSocketTransport({ url: 'ws://localhost:7878' });
    await assert.rejects(
      () => transport.send(new Uint8Array([1, 2, 3])),
      /not connected/,
    );
  });

  it('should call onMessage handler when binary data is received', async () => {
    const transport = new WebSocketTransport({ url: 'ws://localhost:7878', heartbeatIntervalMs: 0 });
    const received: Uint8Array[] = [];
    transport.onMessage((data) => received.push(data));

    const connectPromise = transport.connect();
    const mock = latestMockWs();
    mock.simulateOpen();
    await connectPromise;

    const buffer = new Uint8Array([5, 6, 7]).buffer;
    mock.simulateMessage(buffer);

    assert.equal(received.length, 1, 'handler should fire once');
    assert.deepEqual(received[0], new Uint8Array([5, 6, 7]));
    transport.close();
  });

  it('should not pass string frames to onMessage', async () => {
    const transport = new WebSocketTransport({ url: 'ws://localhost:7878', heartbeatIntervalMs: 0 });
    const received: unknown[] = [];
    transport.onMessage((data) => received.push(data));

    const connectPromise = transport.connect();
    const mock = latestMockWs();
    mock.simulateOpen();
    await connectPromise;

    // Simulate a string frame (heartbeat ack or text message)
    mock.onmessage?.({ data: 'ping' });

    assert.equal(received.length, 0, 'string frames should be ignored');
    transport.close();
  });

  it('should report isConnected = false before connect()', () => {
    const transport = new WebSocketTransport({ url: 'ws://localhost:7878' });
    assert.equal(transport.isConnected, false);
  });

  it('should report isConnected = false after close()', async () => {
    const transport = new WebSocketTransport({ url: 'ws://localhost:7878', heartbeatIntervalMs: 0 });
    const connectPromise = transport.connect();
    const mock = latestMockWs();
    mock.simulateOpen();
    await connectPromise;

    assert.equal(transport.isConnected, true);
    transport.close();
    assert.equal(transport.isConnected, false);
  });

  it('should call onError after max reconnect attempts exhausted', async () => {
    // With maxReconnectAttempts=0, onError fires immediately on first disconnect
    const transport2 = new WebSocketTransport({
      url: 'ws://localhost:7878',
      maxReconnectAttempts: 0,
      reconnectDelayMs: 1,
      heartbeatIntervalMs: 0,
    });
    const errors2: Error[] = [];
    transport2.onError((err) => errors2.push(err));
    const cp2 = transport2.connect();
    const mock2 = latestMockWs();
    mock2.simulateOpen();
    await cp2;

    // Simulate disconnect — with maxReconnectAttempts=0, onError fires immediately
    mock2.simulateClose();

    // Give the synchronous error handler a tick to run
    await Promise.resolve();
    assert.equal(errors2.length, 1, 'onError should fire when reconnect budget exhausted');
    assert.ok(errors2[0]?.message.includes('max reconnect'), `error message: ${errors2[0]?.message}`);
  });

  it('should support multiple onMessage handlers', async () => {
    const transport = new WebSocketTransport({ url: 'ws://localhost:7878', heartbeatIntervalMs: 0 });
    const log1: number[] = [];
    const log2: number[] = [];
    transport.onMessage((d) => log1.push(d[0] ?? 0));
    transport.onMessage((d) => log2.push(d[0] ?? 0));

    const connectPromise = transport.connect();
    const mock = latestMockWs();
    mock.simulateOpen();
    await connectPromise;

    mock.simulateMessage(new Uint8Array([42]).buffer);

    assert.deepEqual(log1, [42]);
    assert.deepEqual(log2, [42]);
    transport.close();
  });

  it('should use default config values when optional fields are omitted', () => {
    // No runtime check of private fields, but verify no error constructing
    const transport = new WebSocketTransport({ url: 'wss://example.com' });
    assert.equal(transport.isConnected, false);
    // If defaults are wrong, connect() would behave incorrectly — this test
    // at least ensures the constructor doesn't throw.
  });
});
