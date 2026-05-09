"""
amaters transactions example (placeholder)
==========================================

The Python SDK does **not** yet expose a transactions API.  The
underlying Rust SDK (``amaters-sdk-rust``) ships a transaction
implementation in ``src/transaction.rs``; surfacing it through the
PyO3 layer is tracked as future work.

Until the dedicated binding lands, the closest equivalent in this
package is to batch reads and writes via :meth:`AmateRSClient.batch`
or :meth:`AmateRSClient.batch_set`, which the server applies as a
single unit.  The example below shows that pattern.

Prerequisites
-------------

A running AmateRS server.  Address is read from ``AMATERS_SERVER`` or
defaults to ``http://localhost:50051``::

    python -m amaters.examples.transactions

See also
--------

* ``crates/amaters-sdk-rust/src/transaction.rs`` — Rust transaction API.
* ``crates/amaters-sdk-python/TODO.md`` — tracking ticket for the
  Python transactions binding.
"""

from __future__ import annotations

import asyncio
import os

from amaters import AmateRSClient


def server_address() -> str:
    """Resolve the server URL from ``AMATERS_SERVER`` or the default."""
    return os.environ.get("AMATERS_SERVER", "http://localhost:50051")


async def main() -> None:
    addr = server_address()
    print(f"[transactions] connecting to {addr}")
    print(
        "[transactions] NOTE: transactions API not yet available from Python; "
        "using batch_set as the closest atomic-write substitute."
    )

    async with AmateRSClient.connect(addr) as client:
        collection = "transactions_demo"

        # ------------------------------------------------------------------
        # Pattern 1 — atomic-style writes via batch_set.  The server
        # processes the batch in a single execution unit.  This is NOT
        # equivalent to a true transaction (no read-your-writes isolation,
        # no abort-and-retry semantics), but it does avoid partial
        # application of the writes from the client's point of view.
        # ------------------------------------------------------------------
        writes = [
            (b"acct:from", b"balance=900"),
            (b"acct:to", b"balance=1100"),
        ]
        stored = await client.batch_set(collection, writes)
        assert stored == len(writes)
        print(f"[transactions] applied {stored} writes as one batch")

        # ------------------------------------------------------------------
        # Pattern 2 — read after write to confirm the batch landed.  Use a
        # heterogeneous batch so the get/get pair is sent in a single
        # round-trip alongside the verification reads.
        # ------------------------------------------------------------------
        results = await client.batch(
            [
                ("get", collection, b"acct:from"),
                ("get", collection, b"acct:to"),
            ]
        )
        assert len(results) == 2
        assert results[0] == b"balance=900"
        assert results[1] == b"balance=1100"
        print("[transactions] verified both balances in a single batch read")

        # ------------------------------------------------------------------
        # Cleanup.
        # ------------------------------------------------------------------
        await client.batch_delete(collection, [b"acct:from", b"acct:to"])
        print("[transactions] cleanup complete.")
        print(
            "[transactions] When a native transactions API ships from the "
            "Python SDK, this example will be rewritten to demonstrate "
            "begin/commit/rollback semantics."
        )


if __name__ == "__main__":
    asyncio.run(main())
