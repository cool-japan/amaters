"""
AmateRS Python SDK
==================

A Python client library for AmateRS — Fully Homomorphic Encrypted Database.

This top-level module is a thin re-export of the compiled PyO3 extension
(``amaters._internal``).  All concrete classes (``AmateRSClient``,
``ClientConfig``, ``RetryConfig``, ``Key``, ``BatchResult``,
``ScanResult``, ``StreamIterator``, ``BatchStreamIterator``) live in
the extension and are surfaced here verbatim.

The companion stub file ``__init__.pyi`` documents every method and is
PEP 561 compliant (see the ``py.typed`` marker beside this file).

Examples
--------

::

    import asyncio
    from amaters import AmateRSClient

    async def main() -> None:
        async with AmateRSClient.connect("http://localhost:50051") as client:
            await client.set("users", b"user:1", b"<ciphertext>")
            value = await client.get("users", b"user:1")
            assert value == b"<ciphertext>"

    asyncio.run(main())

Exception classes
-----------------

The following exception types are re-exported for type-annotation and
catch-block ergonomics.  In practice the PyO3 binding raises
``ConnectionError`` / ``TimeoutError`` / ``ValueError`` /
``RuntimeError`` from the standard library; the project-specific
hierarchy below is provided for code that wants to subclass or document
errors uniformly.
"""

from __future__ import annotations

# ---------------------------------------------------------------------------
# Re-exports from the compiled PyO3 extension module.
#
# ``module-name = "amaters._internal"`` in pyproject.toml ensures the
# extension is registered under that submodule path; importing here makes
# the classes appear at top level under ``amaters``.
# ---------------------------------------------------------------------------

from ._internal import (
    AmateRSClient,
    BatchResult,
    BatchStreamIterator,
    ClientConfig,
    Key,
    RetryConfig,
    ScanResult,
    StreamIterator,
    __version__,
)


# ---------------------------------------------------------------------------
# Project-specific exception hierarchy.
#
# Retained for backwards compatibility with prior releases of this
# package.  The PyO3 layer currently raises the standard-library
# ``ConnectionError`` / ``TimeoutError`` / ``ValueError`` /
# ``RuntimeError`` exception types directly (see
# ``crates/amaters-sdk-python/src/helpers.rs``); these subclasses give
# user code something to ``catch`` and subclass without depending on
# the internal mapping.
# ---------------------------------------------------------------------------


class AmateRSError(Exception):
    """Base exception for AmateRS SDK errors."""


class ConnectionError(AmateRSError):  # noqa: A001 — intentional shadowing for SDK-level type
    """Raised when a connection to the server cannot be established or is lost."""


class TimeoutError(AmateRSError):  # noqa: A001 — intentional shadowing for SDK-level type
    """Raised when an operation exceeds its configured timeout."""


__all__ = [
    "AmateRSClient",
    "ClientConfig",
    "RetryConfig",
    "Key",
    "BatchResult",
    "ScanResult",
    "StreamIterator",
    "BatchStreamIterator",
    "AmateRSError",
    "ConnectionError",
    "TimeoutError",
    "__version__",
]
