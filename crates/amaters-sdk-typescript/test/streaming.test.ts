/**
 * Tests for the streaming async iterator surface (`streamQuery` /
 * `createStreamIterator`).
 *
 * These tests use Node's built-in test runner (`node:test`) and zero-dep
 * `node:assert/strict` matchers — matching the project's `package.json`
 * `test:node` script (`node --test tests/*.test.js`). No vitest/jest.
 *
 * Most coverage drives `createStreamIterator` directly with a fake
 * producer; this lets us exercise backpressure, cancellation, and error
 * propagation without a wasm-pack build. The end-to-end `streamQuery`
 * path is also smoke-tested using the Rust-side stub producer (which
 * lives in TS as `invokeWasmStreamQuery` until the real RPC lands).
 *
 * To run (once a TS loader is wired):
 *   node --import tsx --test test/streaming.test.ts
 *
 * The current CI script (`npm test`) compiles `.ts` → `.js` first; the
 * compiled tests would land in `dist/test/` and be picked up by
 * `node --test`. The compile step is part of the existing build workflow.
 */

import { strict as assert } from 'node:assert';
import { describe, it } from 'node:test';

import {
  createStreamIterator,
  streamQuery,
  type Query,
  type KeyValuePair,
  QueryType,
} from '../src/ts/index';

/**
 * Minimal `Query` stub for the streamQuery smoke test. The shape only
 * needs to satisfy `JSON.stringify` — the streaming side ignores the
 * details until real RPC lands.
 */
function makeStubQuery(collection: string): Query {
  return {
    queryType: QueryType.Range,
    collection,
    updateCount: 0,
    getUpdate: () => null,
  };
}

describe('createStreamIterator', () => {
  it('yields chunks via for-await-of when producer pushes synchronously', async () => {
    const iter = createStreamIterator<string>((onChunk, onDone) => {
      onChunk('a');
      onChunk('b');
      onChunk('c');
      onDone();
    });
    const collected: string[] = [];
    for await (const v of iter) {
      collected.push(v);
    }
    assert.deepEqual(collected, ['a', 'b', 'c']);
  });

  it('yields chunks via for-await-of when producer pushes asynchronously', async () => {
    const iter = createStreamIterator<string>((onChunk, onDone) => {
      // Schedule chunks across microtasks to exercise the waiter path.
      void (async () => {
        await Promise.resolve();
        onChunk('x');
        await Promise.resolve();
        onChunk('y');
        await Promise.resolve();
        onChunk('z');
        onDone();
      })();
    });
    const collected: string[] = [];
    for await (const v of iter) {
      collected.push(v);
    }
    assert.deepEqual(collected, ['x', 'y', 'z']);
  });

  it('drains queued chunks when consumer pulls late', async () => {
    // Producer pushes 3 chunks before consumer pulls — they queue.
    const iter = createStreamIterator<number>((onChunk, onDone) => {
      onChunk(1);
      onChunk(2);
      onChunk(3);
      onDone();
    });
    // Consumer pulls one at a time, exercising the queue drain path.
    const r1 = await iter.next();
    assert.deepEqual(r1, { value: 1, done: false });
    const r2 = await iter.next();
    assert.deepEqual(r2, { value: 2, done: false });
    const r3 = await iter.next();
    assert.deepEqual(r3, { value: 3, done: false });
    const r4 = await iter.next();
    assert.deepEqual(r4, { value: undefined, done: true });
  });

  it('parks consumer pull when no chunk available, resolves on push', async () => {
    let pushChunk: ((v: string) => void) | null = null;
    let signalDone: (() => void) | null = null;
    const iter = createStreamIterator<string>((onChunk, onDone) => {
      pushChunk = onChunk;
      signalDone = onDone;
    });

    // Consumer pulls before producer has emitted — should park.
    const pending = iter.next();
    let settled = false;
    void pending.then(() => {
      settled = true;
    });
    // Microtask flush — the pull is still parked.
    await Promise.resolve();
    assert.equal(settled, false, 'pull should not have settled yet');

    // Producer pushes — pull resolves.
    assert.ok(pushChunk !== null);
    (pushChunk as (v: string) => void)('hello');
    const result = await pending;
    assert.deepEqual(result, { value: 'hello', done: false });

    // Drain.
    assert.ok(signalDone !== null);
    (signalDone as () => void)();
    const final = await iter.next();
    assert.deepEqual(final, { value: undefined, done: true });
  });

  it('cancels via return() and stops yielding further chunks', async () => {
    let pushChunk: ((v: number) => void) | null = null;
    const iter = createStreamIterator<number>((onChunk) => {
      pushChunk = onChunk;
    });

    // Producer emits one chunk.
    assert.ok(pushChunk !== null);
    (pushChunk as (v: number) => void)(1);
    const r1 = await iter.next();
    assert.deepEqual(r1, { value: 1, done: false });

    // Consumer cancels.
    const ret = await iter.return!();
    assert.deepEqual(ret, { value: undefined, done: true });

    // Producer keeps trying to push — chunks must be dropped.
    (pushChunk as (v: number) => void)(2);
    (pushChunk as (v: number) => void)(3);

    // Subsequent pulls all see done.
    const r2 = await iter.next();
    assert.deepEqual(r2, { value: undefined, done: true });
    const r3 = await iter.next();
    assert.deepEqual(r3, { value: undefined, done: true });
  });

  it('cancels via return() drains pending parked pulls', async () => {
    const iter = createStreamIterator<number>(() => {
      // Producer never pushes anything.
    });

    // Two consumers park.
    const p1 = iter.next();
    const p2 = iter.next();

    // Cancel — both should resolve to done.
    const ret = await iter.return!();
    assert.deepEqual(ret, { value: undefined, done: true });

    const r1 = await p1;
    assert.deepEqual(r1, { value: undefined, done: true });
    const r2 = await p2;
    assert.deepEqual(r2, { value: undefined, done: true });
  });

  it('breaking out of for-await-of triggers cancellation cleanly', async () => {
    let cancelled = false;
    const iter = createStreamIterator<number>((onChunk, onDone) => {
      // Push 5 chunks; if the iterator is well-behaved, the consumer
      // should never see all 5 because they break after 2.
      void (async () => {
        for (let i = 1; i <= 5; i++) {
          await Promise.resolve();
          onChunk(i);
        }
        onDone();
      })();
    });
    const seen: number[] = [];
    for await (const v of iter) {
      seen.push(v);
      if (seen.length === 2) {
        cancelled = true;
        break;
      }
    }
    assert.equal(cancelled, true);
    assert.deepEqual(seen, [1, 2]);

    // Iterator is now closed — extra pulls return done.
    const after = await iter.next();
    assert.deepEqual(after, { value: undefined, done: true });
  });

  it('propagates errors from producer onError to pending pulls', async () => {
    let triggerError: ((msg: string) => void) | null = null;
    const iter = createStreamIterator<number>((_onChunk, _onDone, onError) => {
      triggerError = onError;
    });

    // Consumer parks; producer fires error.
    const pending = iter.next();
    assert.ok(triggerError !== null);
    (triggerError as (m: string) => void)('boom');
    await assert.rejects(pending, /boom/);

    // After the first reject, subsequent pulls resolve to done (no
    // double-throw on the same error).
    const after = await iter.next();
    assert.deepEqual(after, { value: undefined, done: true });
  });

  it('propagates errors from producer when fired before consumer pulls', async () => {
    const iter = createStreamIterator<number>((_onChunk, _onDone, onError) => {
      onError('immediate failure');
    });
    // Eager error — first pull rejects.
    await assert.rejects(iter.next(), /immediate failure/);
    // Subsequent pull resolves to done.
    const after = await iter.next();
    assert.deepEqual(after, { value: undefined, done: true });
  });

  it('producer chunks delivered after onDone are dropped', async () => {
    const iter = createStreamIterator<number>((onChunk, onDone) => {
      onChunk(1);
      onDone();
      onChunk(99); // dropped
    });
    const seen: number[] = [];
    for await (const v of iter) {
      seen.push(v);
    }
    assert.deepEqual(seen, [1]);
  });

  it('async iteration is idempotent on Symbol.asyncIterator', () => {
    const iter = createStreamIterator<number>((_o, onDone) => onDone());
    assert.equal(iter[Symbol.asyncIterator](), iter);
  });
});

describe('streamQuery (TS adapter to wasm_stream_query)', () => {
  it('yields the 3-chunk stub stream end-to-end via for-await-of', async () => {
    const q = makeStubQuery('users');
    const seen: KeyValuePair[] = [];
    for await (const kv of streamQuery('http://localhost:50051', 'users', q)) {
      seen.push(kv);
    }
    assert.equal(seen.length, 3);
    // Stub producer emits k1/v1, k2/v2, k3/v3.
    const item0 = seen[0];
    const item1 = seen[1];
    const item2 = seen[2];
    assert.ok(item0 !== undefined);
    assert.ok(item1 !== undefined);
    assert.ok(item2 !== undefined);
    assert.equal(item0.key.toString(), 'k1');
    assert.equal(item1.key.toString(), 'k2');
    assert.equal(item2.key.toString(), 'k3');
    // Values are CipherBlob view types over the bytes of "v1"/"v2"/"v3".
    assert.equal(new TextDecoder().decode(item0.value.toBytes()), 'v1');
    assert.equal(new TextDecoder().decode(item1.value.toBytes()), 'v2');
    assert.equal(new TextDecoder().decode(item2.value.toBytes()), 'v3');
  });

  it('cancels mid-stream via return() and yields no further chunks', async () => {
    const q = makeStubQuery('users');
    const iter = streamQuery('http://localhost:50051', 'users', q);
    const first = await iter.next();
    assert.equal(first.done, false);
    // Cancel before the second chunk is consumed.
    const ret = await iter.return!();
    assert.deepEqual(ret, { value: undefined, done: true });
    // Subsequent pulls all see done. (Note: stub is now async via
    // microtask scheduling — the producer may still try to push chunks
    // 2 and 3 in parallel; they MUST be dropped.)
    const after = await iter.next();
    assert.deepEqual(after, { value: undefined, done: true });
  });

  it('propagates errors when given an empty server URL', async () => {
    const q = makeStubQuery('users');
    const iter = streamQuery('', 'users', q);
    await assert.rejects(iter.next(), /server_url must not be empty/);
  });

  it('propagates errors when given an empty collection', async () => {
    const q = makeStubQuery('users');
    const iter = streamQuery('http://localhost:50051', '', q);
    await assert.rejects(iter.next(), /collection must not be empty/);
  });
});
