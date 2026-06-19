"""
Shared pytest fixtures for the AmateRS Python SDK test suite.

These fixtures rely solely on the Python standard library and pytest —
they do NOT import the compiled ``amaters._internal`` extension, so they
can run even when the wheel has not been built yet (unit-test-only mode).

Fixtures that DO exercise the extension are guarded by the
``amaters_available`` mark: tests decorated with
``@pytest.mark.requires_amaters`` are automatically skipped when the
extension cannot be imported.
"""

from __future__ import annotations

import asyncio
from typing import Any

import pytest

from mocks import (
    MockBatchResult,
    MockClient,
    MockClientConfig,
    MockKey,
    MockRetryConfig,
    MockScanResult,
)

__all__ = [
    "MockBatchResult",
    "MockClient",
    "MockClientConfig",
    "MockKey",
    "MockRetryConfig",
    "MockScanResult",
]

# ---------------------------------------------------------------------------
# Availability guard — try to import the compiled extension; if not built,
# mark it as unavailable without crashing the conftest load.
# ---------------------------------------------------------------------------

try:
    import amaters._internal  # noqa: F401  (side-effect import for availability probe)
    _amaters_available = True
except (ImportError, ModuleNotFoundError):
    _amaters_available = False


def pytest_configure(config: Any) -> None:
    config.addinivalue_line(
        "markers",
        "requires_amaters: skip unless the compiled amaters extension is importable",
    )


def pytest_runtest_setup(item: Any) -> None:
    if item.get_closest_marker("requires_amaters") and not _amaters_available:
        pytest.skip("amaters._internal extension not built — run `maturin develop` first")


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture
def mock_client() -> MockClient:
    """Return a fresh in-memory mock AmateRS client."""
    return MockClient("http://localhost:50051")


@pytest.fixture
def mock_config() -> MockClientConfig:
    return MockClientConfig("http://localhost:50051", timeout_ms=5_000, max_retries=2)


@pytest.fixture
def mock_retry_config() -> MockRetryConfig:
    return MockRetryConfig(max_retries=3, initial_backoff_ms=50, max_backoff_ms=5_000)


@pytest.fixture
def event_loop():
    """Create an event loop scoped to the test session."""
    loop = asyncio.new_event_loop()
    yield loop
    loop.close()
