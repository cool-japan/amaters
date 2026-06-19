"""
Tests for AmateRS client CRUD, batch, range, scan, and prefix operations.

All tests use the in-memory mock client from conftest.py and run
synchronously via asyncio.run() so they work without pytest-asyncio.
"""

from __future__ import annotations

import asyncio
import pytest

from conftest import MockClient


def run(coro):
    return asyncio.get_event_loop().run_until_complete(coro)


# ---------------------------------------------------------------------------
# CRUD operations
# ---------------------------------------------------------------------------

class TestCrudOperations:
    def test_set_and_get(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("users", b"user:1", b"alice"))
        result = run(client.get("users", b"user:1"))
        assert result == b"alice"

    def test_get_missing_key_returns_none(self) -> None:
        client = MockClient("http://localhost:50051")
        result = run(client.get("users", b"nonexistent"))
        assert result is None

    def test_overwrite_existing_key(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col", b"k", b"v1"))
        run(client.set("col", b"k", b"v2"))
        result = run(client.get("col", b"k"))
        assert result == b"v2"

    def test_delete_existing_key(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col", b"k", b"v"))
        run(client.delete("col", b"k"))
        assert run(client.get("col", b"k")) is None

    def test_delete_nonexistent_key_returns_false(self) -> None:
        client = MockClient("http://localhost:50051")
        result = run(client.delete("col", b"ghost"))
        assert result is False

    def test_delete_existing_key_returns_true(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col", b"k", b"v"))
        result = run(client.delete("col", b"k"))
        assert result is True

    def test_contains_true(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col", b"k", b"v"))
        assert run(client.contains("col", b"k")) is True

    def test_contains_false(self) -> None:
        client = MockClient("http://localhost:50051")
        assert run(client.contains("col", b"nope")) is False

    def test_contains_false_after_delete(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col", b"k", b"v"))
        run(client.delete("col", b"k"))
        assert run(client.contains("col", b"k")) is False

    def test_separate_collections_isolated(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col_a", b"key", b"a_val"))
        run(client.set("col_b", b"key", b"b_val"))
        assert run(client.get("col_a", b"key")) == b"a_val"
        assert run(client.get("col_b", b"key")) == b"b_val"

    def test_empty_value(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col", b"k", b""))
        assert run(client.get("col", b"k")) == b""

    def test_binary_key_and_value(self) -> None:
        client = MockClient("http://localhost:50051")
        k = bytes(range(32))
        v = bytes(range(255, 223, -1))
        run(client.set("bin", k, v))
        assert run(client.get("bin", k)) == v


# ---------------------------------------------------------------------------
# Batch operations
# ---------------------------------------------------------------------------

class TestBatchOperations:
    def test_batch_set_and_batch_get(self) -> None:
        client = MockClient("http://localhost:50051")
        pairs = [(b"k1", b"v1"), (b"k2", b"v2"), (b"k3", b"v3")]
        run(client.batch_set("col", pairs))
        result = run(client.batch_get("col", [b"k1", b"k2", b"k3"]))
        assert len(result) == 3
        assert result[0] == b"v1"
        assert result[1] == b"v2"
        assert result[2] == b"v3"

    def test_batch_get_missing_returns_none(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.batch_set("col", [(b"exists", b"yes")]))
        result = run(client.batch_get("col", [b"exists", b"missing"]))
        assert result[0] == b"yes"
        assert result[1] is None

    def test_batch_set_empty(self) -> None:
        client = MockClient("http://localhost:50051")
        result = run(client.batch_set("col", []))
        assert len(result) == 0

    def test_batch_delete(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.batch_set("col", [(b"a", b"1"), (b"b", b"2"), (b"c", b"3")]))
        run(client.batch_delete("col", [b"a", b"c"]))
        assert run(client.get("col", b"a")) is None
        assert run(client.get("col", b"b")) == b"2"
        assert run(client.get("col", b"c")) is None

    def test_batch_delete_empty(self) -> None:
        client = MockClient("http://localhost:50051")
        result = run(client.batch_delete("col", []))
        assert len(result) == 0

    def test_batch_set_large(self) -> None:
        client = MockClient("http://localhost:50051")
        pairs = [(f"key_{i:04}".encode(), f"val_{i}".encode()) for i in range(100)]
        run(client.batch_set("big", pairs))
        result = run(client.batch_get("big", [p[0] for p in pairs]))
        assert len(result) == 100
        assert all(result[i] == f"val_{i}".encode() for i in range(100))


# ---------------------------------------------------------------------------
# Range queries
# ---------------------------------------------------------------------------

class TestRangeQuery:
    def test_range_query_basic(self) -> None:
        client = MockClient("http://localhost:50051")
        for i in range(10):
            run(client.set("r", f"key_{i:02}".encode(), f"val_{i}".encode()))
        pairs = run(client.range_query("r", b"key_03", b"key_07"))
        keys = [p[0] for p in pairs]
        assert b"key_03" in keys
        assert b"key_06" in keys
        assert b"key_07" not in keys  # end is exclusive

    def test_range_query_empty_range(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("r", b"z", b"v"))
        pairs = run(client.range_query("r", b"a", b"b"))
        assert pairs == []

    def test_range_query_with_limit(self) -> None:
        client = MockClient("http://localhost:50051")
        for i in range(20):
            run(client.set("r", f"k{i:02}".encode(), b"v"))
        pairs = run(client.range_query("r", b"k00", b"k20", limit=5))
        assert len(pairs) == 5

    def test_range_query_wrong_collection_empty(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("real", b"k", b"v"))
        pairs = run(client.range_query("fake", b"a", b"z"))
        assert pairs == []


# ---------------------------------------------------------------------------
# Count and keys
# ---------------------------------------------------------------------------

class TestCountAndKeys:
    def test_count_empty(self) -> None:
        client = MockClient("http://localhost:50051")
        assert run(client.count("col")) == 0

    def test_count_after_insert(self) -> None:
        client = MockClient("http://localhost:50051")
        for i in range(5):
            run(client.set("col", f"k{i}".encode(), b"v"))
        assert run(client.count("col")) == 5

    def test_count_after_delete(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col", b"k1", b"v"))
        run(client.set("col", b"k2", b"v"))
        run(client.delete("col", b"k1"))
        assert run(client.count("col")) == 1

    def test_keys_returns_sorted_list(self) -> None:
        client = MockClient("http://localhost:50051")
        for k in [b"c", b"a", b"b"]:
            run(client.set("col", k, b"v"))
        keys = run(client.keys("col"))
        assert keys == [b"a", b"b", b"c"]

    def test_keys_empty_collection(self) -> None:
        client = MockClient("http://localhost:50051")
        assert run(client.keys("empty")) == []


# ---------------------------------------------------------------------------
# Cursor-based scan / pagination
# ---------------------------------------------------------------------------

class TestScan:
    def test_scan_first_page(self) -> None:
        client = MockClient("http://localhost:50051")
        for i in range(10):
            run(client.set("col", f"k{i:02}".encode(), b"v"))
        result = run(client.scan("col", limit=5))
        assert len(result.items) == 5
        assert result.has_more is True

    def test_scan_last_page_has_no_cursor(self) -> None:
        client = MockClient("http://localhost:50051")
        for i in range(3):
            run(client.set("col", f"k{i}".encode(), b"v"))
        result = run(client.scan("col", limit=10))
        assert len(result.items) == 3
        assert result.has_more is False
        assert result.cursor is None

    def test_scan_full_pagination(self) -> None:
        client = MockClient("http://localhost:50051")
        n = 15
        for i in range(n):
            run(client.set("pg", f"key_{i:02}".encode(), f"val_{i}".encode()))
        all_items = []
        cursor = None
        pages = 0
        while True:
            result = run(client.scan("pg", cursor=cursor, limit=5))
            all_items.extend(result.items)
            pages += 1
            if not result.has_more:
                break
            cursor = result.cursor
        assert len(all_items) == n
        assert pages == 3  # 5 + 5 + 5

    def test_scan_empty_collection(self) -> None:
        client = MockClient("http://localhost:50051")
        result = run(client.scan("empty", limit=10))
        assert result.items == []
        assert result.has_more is False


# ---------------------------------------------------------------------------
# Prefix query
# ---------------------------------------------------------------------------

class TestPrefixQuery:
    def test_prefix_query_returns_matching(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col", b"user:1", b"alice"))
        run(client.set("col", b"user:2", b"bob"))
        run(client.set("col", b"post:1", b"hello"))
        results = run(client.prefix_query("col", b"user:"))
        keys = [r[0] for r in results]
        assert b"user:1" in keys
        assert b"user:2" in keys
        assert b"post:1" not in keys

    def test_prefix_query_empty_prefix_returns_all(self) -> None:
        client = MockClient("http://localhost:50051")
        for k in [b"a", b"b", b"c"]:
            run(client.set("col", k, b"v"))
        results = run(client.prefix_query("col", b""))
        assert len(results) == 3

    def test_prefix_query_no_match(self) -> None:
        client = MockClient("http://localhost:50051")
        run(client.set("col", b"x:1", b"v"))
        results = run(client.prefix_query("col", b"y:"))
        assert results == []


# ---------------------------------------------------------------------------
# Pool stats
# ---------------------------------------------------------------------------

class TestPoolStats:
    def test_pool_stats_returns_dict(self) -> None:
        client = MockClient("http://localhost:50051")
        stats = run(client.pool_stats())
        assert isinstance(stats, dict)
        assert "total_connections" in stats
        assert "active_connections" in stats
        assert "idle_connections" in stats
        assert "max_connections" in stats

    def test_pool_stats_values_are_non_negative(self) -> None:
        client = MockClient("http://localhost:50051")
        stats = run(client.pool_stats())
        assert all(v >= 0 for v in stats.values())


# ---------------------------------------------------------------------------
# Context manager / lifecycle
# ---------------------------------------------------------------------------

class TestLifecycle:
    def test_async_context_manager(self) -> None:
        async def _inner():
            async with MockClient("http://localhost:50051") as client:
                await client.set("col", b"k", b"v")
                assert await client.get("col", b"k") == b"v"
            assert client._closed is True

        run(_inner())

    def test_close_marks_client_closed(self) -> None:
        client = MockClient("http://localhost:50051")
        assert client._closed is False
        run(client.close())
        assert client._closed is True
