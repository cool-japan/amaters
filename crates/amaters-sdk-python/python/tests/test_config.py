"""
Tests for ClientConfig and RetryConfig types.

These tests exercise the Python-level configuration API. Mock classes are
used so the suite runs without the compiled extension.
"""

from __future__ import annotations

import pytest

from conftest import MockClientConfig, MockRetryConfig


class TestClientConfig:
    def test_default_timeout_ms(self) -> None:
        cfg = MockClientConfig("http://localhost:50051")
        assert cfg.timeout_ms == 30_000

    def test_default_max_retries(self) -> None:
        cfg = MockClientConfig("http://localhost:50051")
        assert cfg.max_retries == 3

    def test_custom_timeout(self) -> None:
        cfg = MockClientConfig("http://localhost:50051", timeout_ms=5_000)
        assert cfg.timeout_ms == 5_000

    def test_custom_max_retries(self) -> None:
        cfg = MockClientConfig("http://localhost:50051", max_retries=0)
        assert cfg.max_retries == 0

    def test_endpoint_stored(self) -> None:
        ep = "https://db.example.com:50051"
        cfg = MockClientConfig(ep)
        assert cfg.endpoint == ep

    def test_repr_contains_endpoint(self) -> None:
        cfg = MockClientConfig("http://localhost:50051")
        assert "localhost:50051" in repr(cfg)

    def test_repr_contains_timeout(self) -> None:
        cfg = MockClientConfig("http://localhost:50051", timeout_ms=1234)
        assert "1234" in repr(cfg)

    def test_repr_contains_max_retries(self) -> None:
        cfg = MockClientConfig("http://localhost:50051", max_retries=7)
        assert "7" in repr(cfg)

    def test_zero_timeout_is_valid(self) -> None:
        cfg = MockClientConfig("http://localhost:50051", timeout_ms=0)
        assert cfg.timeout_ms == 0

    def test_large_timeout_is_valid(self) -> None:
        cfg = MockClientConfig("http://localhost:50051", timeout_ms=300_000)
        assert cfg.timeout_ms == 300_000


class TestRetryConfig:
    def test_default_max_retries(self) -> None:
        cfg = MockRetryConfig()
        assert cfg.max_retries == 3

    def test_default_initial_backoff(self) -> None:
        cfg = MockRetryConfig()
        assert cfg.initial_backoff_ms == 100

    def test_default_max_backoff(self) -> None:
        cfg = MockRetryConfig()
        assert cfg.max_backoff_ms == 10_000

    def test_default_multiplier(self) -> None:
        cfg = MockRetryConfig()
        assert cfg.backoff_multiplier == 2.0

    def test_no_retry_factory(self) -> None:
        cfg = MockRetryConfig.no_retry()
        assert cfg.max_retries == 0

    def test_no_retry_zero_backoff(self) -> None:
        cfg = MockRetryConfig.no_retry()
        assert cfg.initial_backoff_ms == 0
        assert cfg.max_backoff_ms == 0

    def test_custom_values(self) -> None:
        cfg = MockRetryConfig(
            max_retries=10,
            initial_backoff_ms=50,
            max_backoff_ms=60_000,
            backoff_multiplier=1.5,
        )
        assert cfg.max_retries == 10
        assert cfg.initial_backoff_ms == 50
        assert cfg.max_backoff_ms == 60_000
        assert cfg.backoff_multiplier == 1.5

    def test_repr_contains_max_retries(self) -> None:
        cfg = MockRetryConfig(max_retries=5)
        assert "5" in repr(cfg)

    def test_repr_contains_backoff(self) -> None:
        cfg = MockRetryConfig(initial_backoff_ms=200)
        assert "200" in repr(cfg)

    def test_multiplier_of_one_is_linear(self) -> None:
        cfg = MockRetryConfig(max_retries=3, backoff_multiplier=1.0)
        assert cfg.backoff_multiplier == 1.0
