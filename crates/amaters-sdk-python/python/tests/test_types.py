"""
Tests for SDK value types: Key, BatchResult, ScanResult.
"""

from __future__ import annotations

import pytest

from conftest import MockBatchResult, MockKey, MockScanResult


class TestKey:
    def test_bytes_round_trip(self) -> None:
        k = MockKey(b"hello")
        assert bytes(k) == b"hello"

    def test_len(self) -> None:
        k = MockKey(b"abc")
        assert len(k) == 3

    def test_empty_key(self) -> None:
        k = MockKey(b"")
        assert len(k) == 0
        assert bytes(k) == b""

    def test_equality(self) -> None:
        a = MockKey(b"key")
        b = MockKey(b"key")
        assert a == b

    def test_inequality(self) -> None:
        a = MockKey(b"key1")
        b = MockKey(b"key2")
        assert a != b

    def test_hash_equal_keys_same_hash(self) -> None:
        a = MockKey(b"k")
        b = MockKey(b"k")
        assert hash(a) == hash(b)

    def test_hash_different_keys_different_hash(self) -> None:
        a = MockKey(b"foo")
        b = MockKey(b"bar")
        assert hash(a) != hash(b)

    def test_repr_contains_data(self) -> None:
        k = MockKey(b"mykey")
        assert "mykey" in repr(k)

    def test_str_round_trip_ascii(self) -> None:
        k = MockKey(b"ascii_key")
        assert str(k) == "ascii_key"

    def test_set_membership(self) -> None:
        keys = {MockKey(b"a"), MockKey(b"b"), MockKey(b"c")}
        assert MockKey(b"a") in keys
        assert MockKey(b"d") not in keys

    def test_binary_key(self) -> None:
        raw = bytes(range(16))
        k = MockKey(raw)
        assert bytes(k) == raw


class TestBatchResult:
    def test_empty_batch(self) -> None:
        br = MockBatchResult([])
        assert len(br) == 0

    def test_len(self) -> None:
        br = MockBatchResult([1, 2, 3])
        assert len(br) == 3

    def test_index_access(self) -> None:
        br = MockBatchResult(["a", "b", "c"])
        assert br[0] == "a"
        assert br[2] == "c"

    def test_iteration(self) -> None:
        items = [10, 20, 30]
        br = MockBatchResult(items)
        assert list(br) == items

    def test_repr_contains_items(self) -> None:
        br = MockBatchResult([1, 2])
        assert "1" in repr(br)

    def test_single_item(self) -> None:
        br = MockBatchResult([{"success": True}])
        assert len(br) == 1
        assert br[0]["success"] is True

    def test_nested_results(self) -> None:
        br = MockBatchResult([b"val1", None, b"val2"])
        assert br[0] == b"val1"
        assert br[1] is None
        assert br[2] == b"val2"


class TestScanResult:
    def test_no_cursor_no_more(self) -> None:
        sr = MockScanResult([(b"k", b"v")])
        assert sr.has_more is False
        assert sr.cursor is None

    def test_cursor_present_has_more(self) -> None:
        sr = MockScanResult([(b"k1", b"v1"), (b"k2", b"v2")], cursor=b"k2")
        assert sr.has_more is True
        assert sr.cursor == b"k2"

    def test_items_preserved(self) -> None:
        pairs = [(b"a", b"1"), (b"b", b"2")]
        sr = MockScanResult(pairs)
        assert sr.items == pairs

    def test_empty_items(self) -> None:
        sr = MockScanResult([])
        assert len(sr.items) == 0
        assert sr.has_more is False

    def test_repr_contains_item_count(self) -> None:
        sr = MockScanResult([(b"k", b"v")] * 5)
        assert "5" in repr(sr)

    def test_repr_has_more_false(self) -> None:
        sr = MockScanResult([])
        assert "False" in repr(sr)

    def test_repr_has_more_true(self) -> None:
        sr = MockScanResult([(b"k", b"v")], cursor=b"k")
        assert "True" in repr(sr)
