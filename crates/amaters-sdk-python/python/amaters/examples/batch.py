"""
amaters batch operations example
================================

Demonstrates the batch APIs (:meth:`AmateRSClient.batch_set`,
:meth:`AmateRSClient.batch_get`, :meth:`AmateRSClient.batch_delete`,
and the heterogeneous :meth:`AmateRSClient.batch`).

Prerequisites
-------------

A running AmateRS server.  Address is read from ``AMATERS_SERVER`` or
defaults to ``http://localhost:50051``::

    python -m amaters.examples.batch
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
    print(f"[batch] connecting to {addr}")

    async with AmateRSClient.connect(addr) as client:
        collection = "batch_demo"

        # -- batch_set ------------------------------------------------------
        items = [
            (f"key:{i}".encode(), f"value:{i}".encode())
            for i in range(10)
        ]
        stored = await client.batch_set(collection, items)
        assert stored == len(items)
        print(f"[batch] batch_set stored {stored} entries")

        # -- batch_get ------------------------------------------------------
        keys = [k for k, _ in items]
        results = await client.batch_get(collection, keys)
        assert len(results) == len(keys)
        for key, value in results:
            assert value is not None, f"missing value for {key!r}"
        print(f"[batch] batch_get returned {len(results)} pairs")

        # -- heterogeneous batch -------------------------------------------
        ops = [
            ("set", collection, b"user:42", b"answer-ciphertext"),
            ("get", collection, b"user:42"),
            ("get", collection, b"key:0"),
            ("delete", collection, b"key:0"),
        ]
        outcomes = await client.batch(ops)
        assert len(outcomes) == len(ops)
        # Indexes 1 and 2 are gets; 0 and 3 are set/delete (return None).
        assert outcomes[1] == b"answer-ciphertext"
        assert outcomes[2] == b"value:0"
        print("[batch] heterogeneous batch completed")

        # -- batch_delete --------------------------------------------------
        # Delete the remaining keys (key:1..key:9 — key:0 was deleted above).
        remaining = [k for k, _ in items[1:]]
        deleted = await client.batch_delete(collection, remaining)
        assert deleted == len(remaining)
        print(f"[batch] batch_delete removed {deleted} entries")

        # Cleanup the auxiliary key from the heterogeneous batch.
        await client.delete(collection, b"user:42")

        print("[batch] done.")


if __name__ == "__main__":
    asyncio.run(main())
