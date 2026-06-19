"""
Property-based tests for the AmateRS Python SDK using Hypothesis.

These tests verify invariants that must hold for ANY valid key/value input,
not just hand-crafted examples.  Each test encodes a semantic contract:

  - get(put(k, v)) == v               (get-after-put)
  - get(delete(k)) == None            (get-after-delete)
  - count == len(keys())              (count consistency)
  - scan pagination covers all keys   (pagination completeness)
  - prefix_query ⊆ keys               (prefix correctness)
  - range(k, k) == []                 (empty half-open range)

The tests use the in-memory mock client from conftest.py so they run
without the compiled extension.
"""

from __future__ import annotations

import asyncio
from typing import Any

import pytest

try:
    from hypothesis import given, settings, assume
    from hypothesis import strategies as st

    HYPOTHESIS_AVAILABLE = True
except ImportError:
    HYPOTHESIS_AVAILABLE = False

from conftest import MockClient

pytestmark = pytest.mark.skipif(
    not HYPOTHESIS_AVAILABLE,
    reason="hypothesis not installed — run: pip install hypothesis",
)


def run(coro):
    return asyncio.get_event_loop().run_until_complete(coro)


# ---------------------------------------------------------------------------
# Strategies
# ---------------------------------------------------------------------------

# Keys: arbitrary non-empty byte strings up to 128 bytes
key_strategy = st.binary(min_size=1, max_size=128)

# Values: arbitrary byte strings (including empty) up to 1024 bytes
value_strategy = st.binary(min_size=0, max_size=1024)

# Small collections of distinct keys
distinct_keys_strategy = st.lists(
    key_strategy, min_size=1, max_size=10, unique=True
)


# ---------------------------------------------------------------------------
# Invariant 1: get-after-put
# ---------------------------------------------------------------------------


@given(key=key_strategy, value=value_strategy)
@settings(max_examples=50, deadline=None)
def test_get_after_put_returns_value(key: bytes, value: bytes) -> None:
    client = MockClient("http://localhost:50051")
    run(client.set("col", key, value))
    got = run(client.get("col", key))
    assert got == value, f"get({key!r}) returned {got!r}, expected {value!r}"


# ---------------------------------------------------------------------------
# Invariant 2: get-after-delete returns None
# ---------------------------------------------------------------------------


@given(key=key_strategy, value=value_strategy)
@settings(max_examples=50, deadline=None)
def test_get_after_delete_returns_none(key: bytes, value: bytes) -> None:
    client = MockClient("http://localhost:50051")
    run(client.set("col", key, value))
    run(client.delete("col", key))
    got = run(client.get("col", key))
    assert got is None, f"get after delete returned {got!r}, expected None"


# ---------------------------------------------------------------------------
# Invariant 3: overwrite is idempotent for the value
# ---------------------------------------------------------------------------


@given(key=key_strategy, val1=value_strategy, val2=value_strategy)
@settings(max_examples=50, deadline=None)
def test_overwrite_returns_latest_value(
    key: bytes, val1: bytes, val2: bytes
) -> None:
    client = MockClient("http://localhost:50051")
    run(client.set("col", key, val1))
    run(client.set("col", key, val2))
    got = run(client.get("col", key))
    assert got == val2, f"overwrite: expected {val2!r}, got {got!r}"


# ---------------------------------------------------------------------------
# Invariant 4: count == len(keys())
# ---------------------------------------------------------------------------


@given(pairs=st.lists(st.tuples(key_strategy, value_strategy), min_size=0, max_size=10, unique_by=lambda p: p[0]))
@settings(max_examples=50, deadline=None)
def test_count_matches_keys_length(pairs: list[tuple[bytes, bytes]]) -> None:
    client = MockClient("http://localhost:50051")
    for k, v in pairs:
        run(client.set("col", k, v))
    count = run(client.count("col"))
    keys = run(client.keys("col"))
    assert count == len(keys), (
        f"count={count} != len(keys)={len(keys)}"
    )


# ---------------------------------------------------------------------------
# Invariant 5: contains is consistent with get
# ---------------------------------------------------------------------------


@given(key=key_strategy, value=value_strategy, present=st.booleans())
@settings(max_examples=50, deadline=None)
def test_contains_consistent_with_get(
    key: bytes, value: bytes, present: bool
) -> None:
    client = MockClient("http://localhost:50051")
    if present:
        run(client.set("col", key, value))
    else:
        # Make sure the key is absent
        run(client.delete("col", key))

    exists = run(client.contains("col", key))
    got = run(client.get("col", key))

    if present:
        assert exists, "contains should be True for an inserted key"
        assert got is not None, "get should return a value for an inserted key"
    else:
        assert not exists, "contains should be False for an absent key"
        assert got is None, "get should return None for an absent key"


# ---------------------------------------------------------------------------
# Invariant 6: range(k, k) is always empty (half-open interval)
# ---------------------------------------------------------------------------


@given(key=key_strategy, value=value_strategy)
@settings(max_examples=50, deadline=None)
def test_empty_half_open_range(key: bytes, value: bytes) -> None:
    client = MockClient("http://localhost:50051")
    run(client.set("col", key, value))
    result = run(client.range_query("col", key, key))
    assert result == [], (
        f"range({key!r}, {key!r}) should be empty, got {result!r}"
    )


# ---------------------------------------------------------------------------
# Invariant 7: prefix_query ⊆ keys()
# ---------------------------------------------------------------------------


@given(pairs=distinct_keys_strategy.flatmap(
    lambda ks: st.fixed_dictionaries({k: value_strategy for k in ks})
), prefix=key_strategy)
@settings(max_examples=30, deadline=None)
def test_prefix_query_subset_of_keys(
    pairs: dict[bytes, bytes], prefix: bytes
) -> None:
    client = MockClient("http://localhost:50051")
    for k, v in pairs.items():
        run(client.set("col", k, v))

    prefix_results = run(client.prefix_query("col", prefix))
    all_keys = set(run(client.keys("col")))

    for k, _v in prefix_results:
        assert k in all_keys, f"prefix result key {k!r} not in all keys"
        assert k.startswith(prefix), (
            f"prefix result {k!r} does not start with prefix {prefix!r}"
        )


# ---------------------------------------------------------------------------
# Invariant 8: scan pagination covers exactly all keys (no duplicates, no drops)
# ---------------------------------------------------------------------------


@given(pairs=distinct_keys_strategy.flatmap(
    lambda ks: st.fixed_dictionaries({k: value_strategy for k in ks})
), page_size=st.integers(min_value=1, max_value=5))
@settings(max_examples=30, deadline=None)
def test_scan_pagination_complete_and_no_duplicates(
    pairs: dict[bytes, bytes], page_size: int
) -> None:
    client = MockClient("http://localhost:50051")
    for k, v in pairs.items():
        run(client.set("col", k, v))

    all_expected = sorted(pairs.keys())

    # Paginate through all keys
    collected = []
    cursor = None
    while True:
        result = run(client.scan("col", cursor=cursor, limit=page_size))
        for k, _v in result.items:
            collected.append(k)
        if not result.has_more:
            break
        cursor = result.cursor

    assert sorted(collected) == all_expected, (
        f"pagination mismatch: collected={sorted(collected)!r}, "
        f"expected={all_expected!r}"
    )
    assert len(collected) == len(set(collected)), "pagination returned duplicates"


# ---------------------------------------------------------------------------
# Invariant 9: batch_set then batch_get roundtrip
# ---------------------------------------------------------------------------


@given(pairs=distinct_keys_strategy.flatmap(
    lambda ks: st.fixed_dictionaries({k: value_strategy for k in ks})
))
@settings(max_examples=30, deadline=None)
def test_batch_set_then_batch_get_roundtrip(
    pairs: dict[bytes, bytes]
) -> None:
    client = MockClient("http://localhost:50051")
    items = list(pairs.items())
    run(client.batch_set("col", items))
    keys = [k for k, _ in items]
    results = run(client.batch_get("col", keys))
    for i, (k, expected_v) in enumerate(items):
        assert results[i] == expected_v, (
            f"batch roundtrip failed at key {k!r}: got {results[i]!r}"
        )


# ---------------------------------------------------------------------------
# Invariant 10: delete reduces count by exactly 1 for a present key
# ---------------------------------------------------------------------------


@given(key=key_strategy, value=value_strategy)
@settings(max_examples=50, deadline=None)
def test_delete_reduces_count_by_one(key: bytes, value: bytes) -> None:
    client = MockClient("http://localhost:50051")
    run(client.set("col", key, value))
    before = run(client.count("col"))
    run(client.delete("col", key))
    after = run(client.count("col"))
    assert after == before - 1, (
        f"count before delete={before}, after delete={after}; expected {before - 1}"
    )
