"""
Tests for error handling and edge cases in the AmateRS Python SDK.

Verifies that the mock client (and by analogy the real client) correctly
handles boundary conditions: empty collections, large payloads, concurrent
access, and re-use after close.
"""

from __future__ import annotations

import asyncio
import pytest

from conftest import MockClient, MockClientConfig, MockRetryConfig


def run(coro):
    return asyncio.get_event_loop().run_until_complete(coro)


# ---------------------------------------------------------------------------
# Edge cases for key and value handling
# ---------------------------------------------------------------------------

class TestKeyEdgeCases:
    def test_zero_byte_key(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col", b"\x00", b"val"))
        assert run(client.get("col", b"\x00")) == b"val"

    def test_null_bytes_in_key(self) -> None:
        client = MockClient("http://localhost:50051")
        k = b"key\x00with\x00nulls"
        run(client.set("col", k, b"val"))
        assert run(client.get("col", k)) == b"val"

    def test_very_long_key(self) -> None:
        client = MockClient("http://localhost:50051")
        k = b"x" * 4096
        run(client.set("col", k, b"val"))
        assert run(client.get("col", k)) == b"val"

    def test_very_large_value(self) -> None:
        client = MockClient("http://localhost:50051")
        v = bytes(range(256)) * 1000  # 256 KB
        run(client.set("col", b"k", v))
        assert run(client.get("col", b"k")) == v

    def test_high_unicode_utf8_key(self) -> None:
        client = MockClient("http://localhost:50051")
        k = "héllo wörld 日本語".encode("utf-8")
        v = "encryption_result".encode("utf-8")
        run(client.set("col", k, v))
        assert run(client.get("col", k)) == v


# ---------------------------------------------------------------------------
# Concurrent access patterns
# ---------------------------------------------------------------------------

class TestConcurrentAccess:
    def test_concurrent_sets(self) -> None:
        async def _inner():
            client = MockClient("http://localhost:50051")
            tasks = [
                client.set("col", f"key_{i:04}".encode(), f"val_{i}".encode())
                for i in range(20)
            ]
            await asyncio.gather(*tasks)
            assert await client.count("col") == 20

        run(_inner())

    def test_concurrent_gets(self) -> None:
        async def _inner():
            client = MockClient("http://localhost:50051")
            for i in range(10):
                await client.set("col", f"k{i}".encode(), f"v{i}".encode())
            results = await asyncio.gather(
                *[client.get("col", f"k{i}".encode()) for i in range(10)]
            )
            assert all(r == f"v{i}".encode() for i, r in enumerate(results))

        run(_inner())

    def test_concurrent_batch_sets(self) -> None:
        async def _inner():
            client = MockClient("http://localhost:50051")
            batches = [
                [(f"k{batch}_{i:02}".encode(), b"v") for i in range(5)]
                for batch in range(4)
            ]
            await asyncio.gather(*[client.batch_set("col", b) for b in batches])
            assert await client.count("col") == 20

        run(_inner())


# ---------------------------------------------------------------------------
# Idempotency and re-use
# ---------------------------------------------------------------------------

class TestIdempotency:
    def test_delete_twice_is_idempotent(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col", b"k", b"v"))
        assert run(client.delete("col", b"k")) is True
        assert run(client.delete("col", b"k")) is False

    def test_set_same_key_multiple_times(self) -> None:
        client = MockClient("http://localhost:50051")
        for i in range(5):
            run(client.set("col", b"k", f"v{i}".encode()))
        assert run(client.get("col", b"k")) == b"v4"

    def test_empty_batch_set_no_side_effects(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col", b"existing", b"v"))
        run(client.batch_set("col", []))
        assert run(client.count("col")) == 1
        assert run(client.get("col", b"existing")) == b"v"


# ---------------------------------------------------------------------------
# Collection isolation
# ---------------------------------------------------------------------------

class TestCollectionIsolation:
    def test_same_key_different_collections(self) -> None:
        client = MockClient("http://localhost:50051")
        for col in ["a", "b", "c"]:
            run(client.set(col, b"shared_key", f"val_{col}".encode()))
        for col in ["a", "b", "c"]:
            assert run(client.get(col, b"shared_key")) == f"val_{col}".encode()

    def test_delete_in_one_collection_does_not_affect_others(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col_a", b"k", b"v"))
        run(client.set("col_b", b"k", b"v"))
        run(client.delete("col_a", b"k"))
        assert run(client.get("col_a", b"k")) is None
        assert run(client.get("col_b", b"k")) == b"v"

    def test_count_is_per_collection(self) -> None:
        client = MockClient("http://localhost:50051")
        for i in range(3):
            run(client.set("col_a", f"k{i}".encode(), b"v"))
        for i in range(7):
            run(client.set("col_b", f"k{i}".encode(), b"v"))
        assert run(client.count("col_a")) == 3
        assert run(client.count("col_b")) == 7


# ---------------------------------------------------------------------------
# Pagination edge cases
# ---------------------------------------------------------------------------

class TestPaginationEdgeCases:
    def test_scan_single_item(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col", b"only_key", b"val"))
        result = run(client.scan("col", limit=10))
        assert len(result.items) == 1
        assert result.has_more is False

    def test_scan_limit_equals_count(self) -> None:
        client = MockClient("http://localhost:50051")
        for i in range(5):
            run(client.set("col", f"k{i}".encode(), b"v"))
        result = run(client.scan("col", limit=5))
        assert len(result.items) == 5
        assert result.has_more is False

    def test_scan_limit_one(self) -> None:
        client = MockClient("http://localhost:50051")
        for i in range(3):
            run(client.set("col", f"k{i}".encode(), b"v"))
        result = run(client.scan("col", limit=1))
        assert len(result.items) == 1
        assert result.has_more is True

    def test_scan_all_pages_cover_all_keys(self) -> None:
        client = MockClient("http://localhost:50051")
        n = 23
        expected = sorted([f"key_{i:03}".encode() for i in range(n)])
        for k in expected:
            run(client.set("col", k, b"v"))
        collected_keys = []
        cursor = None
        while True:
            result = run(client.scan("col", cursor=cursor, limit=7))
            collected_keys.extend(k for k, _ in result.items)
            if not result.has_more:
                break
            cursor = result.cursor
        assert sorted(collected_keys) == expected


# ---------------------------------------------------------------------------
# Pool stats
# ---------------------------------------------------------------------------

class TestPoolStatsEdgeCases:
    def test_pool_stats_idle_plus_active_le_total(self) -> None:
        client = MockClient("http://localhost:50051")
        stats = run(client.pool_stats())
        assert stats["idle_connections"] + stats["active_connections"] <= stats["total_connections"]

    def test_pool_stats_total_le_max(self) -> None:
        client = MockClient("http://localhost:50051")
        stats = run(client.pool_stats())
        assert stats["total_connections"] <= stats["max_connections"]


# ---------------------------------------------------------------------------
# Range query edge cases
# ---------------------------------------------------------------------------

class TestRangeQueryEdgeCases:
    def test_range_query_start_equals_end_returns_empty(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col", b"k", b"v"))
        results = run(client.range_query("col", b"k", b"k"))
        assert results == []

    def test_range_query_start_after_all_keys_returns_empty(self) -> None:
        client = MockClient("http://localhost:50051")
        for i in range(5):
            run(client.set("col", f"key_{i}".encode(), b"v"))
        results = run(client.range_query("col", b"zzz", b"zzzzz"))
        assert results == []

    def test_range_query_respects_lexicographic_order(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col", b"10", b"ten"))
        run(client.set("col", b"2", b"two"))
        run(client.set("col", b"20", b"twenty"))
        # In lexicographic order: "10" < "2" < "20"
        results = run(client.range_query("col", b"10", b"20"))
        keys = [r[0] for r in results]
        assert b"10" in keys
        assert b"2" in keys
        assert b"20" not in keys  # end is exclusive
