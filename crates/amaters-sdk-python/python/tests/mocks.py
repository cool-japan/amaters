"""
In-memory mock classes for AmateRS Python SDK tests.

These stand-ins mirror the public API of ``amaters.AmateRSClient`` and
related types so the test suite can run without the compiled extension.
"""

from __future__ import annotations

import asyncio
from typing import Any


class MockKey:
    """Minimal stand-in for ``amaters.Key``."""

    def __init__(self, data: bytes) -> None:
        self._data = data

    def __bytes__(self) -> bytes:
        return self._data

    def __len__(self) -> int:
        return len(self._data)

    def __eq__(self, other: object) -> bool:
        if isinstance(other, MockKey):
            return self._data == other._data
        return NotImplemented

    def __hash__(self) -> int:
        return hash(self._data)

    def __repr__(self) -> str:
        return f"Key({self._data!r})"

    def __str__(self) -> str:
        return self._data.decode("utf-8", errors="replace")


class MockClientConfig:
    """Minimal stand-in for ``amaters.ClientConfig``."""

    def __init__(
        self,
        endpoint: str,
        timeout_ms: int = 30_000,
        max_retries: int = 3,
    ) -> None:
        self.endpoint = endpoint
        self.timeout_ms = timeout_ms
        self.max_retries = max_retries

    def __repr__(self) -> str:
        return (
            f"ClientConfig(endpoint={self.endpoint!r}, "
            f"timeout_ms={self.timeout_ms}, max_retries={self.max_retries})"
        )


class MockRetryConfig:
    """Minimal stand-in for ``amaters.RetryConfig``."""

    def __init__(
        self,
        max_retries: int = 3,
        initial_backoff_ms: int = 100,
        max_backoff_ms: int = 10_000,
        backoff_multiplier: float = 2.0,
    ) -> None:
        self.max_retries = max_retries
        self.initial_backoff_ms = initial_backoff_ms
        self.max_backoff_ms = max_backoff_ms
        self.backoff_multiplier = backoff_multiplier

    @classmethod
    def no_retry(cls) -> "MockRetryConfig":
        return cls(max_retries=0, initial_backoff_ms=0, max_backoff_ms=0, backoff_multiplier=1.0)

    def __repr__(self) -> str:
        return (
            f"RetryConfig(max_retries={self.max_retries}, "
            f"initial_backoff_ms={self.initial_backoff_ms}, "
            f"max_backoff_ms={self.max_backoff_ms}, "
            f"backoff_multiplier={self.backoff_multiplier})"
        )


class MockBatchResult:
    """Minimal stand-in for ``amaters.BatchResult``."""

    def __init__(self, items: list[Any]) -> None:
        self._items = items

    def __len__(self) -> int:
        return len(self._items)

    def __iter__(self):
        return iter(self._items)

    def __getitem__(self, idx: int) -> Any:
        return self._items[idx]

    def __repr__(self) -> str:
        return f"BatchResult({self._items!r})"


class MockScanResult:
    """Minimal stand-in for ``amaters.ScanResult``."""

    def __init__(self, items: list[tuple[bytes, bytes]], cursor: bytes | None = None) -> None:
        self.items = items
        self.cursor = cursor
        self.has_more = cursor is not None

    def __repr__(self) -> str:
        return f"ScanResult(items={len(self.items)}, has_more={self.has_more})"


class MockClient:
    """Minimal async stand-in for ``amaters.AmateRSClient``.

    Backed by an in-memory ``dict`` so tests can verify round-trip
    semantics without a real server.
    """

    def __init__(self, endpoint: str, config: MockClientConfig | None = None) -> None:
        self._endpoint = endpoint
        self._config = config
        self._store: dict[tuple[str, bytes], bytes] = {}
        self._closed = False

    # ---- context manager ----

    async def __aenter__(self) -> "MockClient":
        return self

    async def __aexit__(self, *_: Any) -> None:
        await self.close()

    # ---- lifecycle ----

    @classmethod
    async def connect(
        cls,
        endpoint: str,
        config: MockClientConfig | None = None,
    ) -> "MockClient":
        return cls(endpoint, config)

    async def close(self) -> None:
        self._closed = True

    # ---- CRUD ----

    async def set(self, collection: str, key: bytes, value: bytes) -> None:
        self._store[(collection, key)] = value

    async def get(self, collection: str, key: bytes) -> bytes | None:
        return self._store.get((collection, key))

    async def delete(self, collection: str, key: bytes) -> bool:
        existed = (collection, key) in self._store
        self._store.pop((collection, key), None)
        return existed

    async def contains(self, collection: str, key: bytes) -> bool:
        return (collection, key) in self._store

    # ---- batch ----

    async def batch_set(
        self, collection: str, pairs: list[tuple[bytes, bytes]]
    ) -> MockBatchResult:
        for k, v in pairs:
            self._store[(collection, k)] = v
        items = [{"success": True} for _ in pairs]
        return MockBatchResult(items)

    async def batch_get(
        self, collection: str, keys: list[bytes]
    ) -> MockBatchResult:
        items = [self._store.get((collection, k)) for k in keys]
        return MockBatchResult(items)

    async def batch_delete(
        self, collection: str, keys: list[bytes]
    ) -> MockBatchResult:
        results = []
        for k in keys:
            existed = (collection, k) in self._store
            self._store.pop((collection, k), None)
            results.append({"success": True, "affected": 1 if existed else 0})
        return MockBatchResult(results)

    # ---- range ----

    async def range_query(
        self,
        collection: str,
        start: bytes,
        end: bytes,
        limit: int = 100,
    ) -> list[tuple[bytes, bytes]]:
        results = []
        for (col, k), v in sorted(self._store.items()):
            if col == collection and start <= k < end:
                results.append((k, v))
                if len(results) >= limit:
                    break
        return results

    # ---- count / keys ----

    async def count(self, collection: str) -> int:
        return sum(1 for (col, _) in self._store if col == collection)

    async def keys(self, collection: str) -> list[bytes]:
        return [k for (col, k) in sorted(self._store) if col == collection]

    # ---- scan / cursor pagination ----

    async def scan(
        self,
        collection: str,
        cursor: bytes | None = None,
        limit: int = 10,
    ) -> MockScanResult:
        all_keys = sorted(k for (col, k) in self._store if col == collection)
        if cursor is not None:
            try:
                start_idx = all_keys.index(cursor) + 1
            except ValueError:
                start_idx = 0
        else:
            start_idx = 0
        page = all_keys[start_idx : start_idx + limit]
        items = [(k, self._store[(collection, k)]) for k in page]
        next_cursor = (
            page[-1] if len(page) == limit and start_idx + limit < len(all_keys) else None
        )
        return MockScanResult(items, next_cursor)

    # ---- pool stats ----

    async def pool_stats(self) -> dict[str, int]:
        return {
            "total_connections": 1,
            "active_connections": 0,
            "idle_connections": 1,
            "max_connections": 10,
        }

    # ---- prefix ----

    async def prefix_query(
        self, collection: str, prefix: bytes
    ) -> list[tuple[bytes, bytes]]:
        return [
            (k, v)
            for (col, k), v in sorted(self._store.items())
            if col == collection and k.startswith(prefix)
        ]
