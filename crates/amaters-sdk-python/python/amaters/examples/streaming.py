"""
amaters streaming example
=========================

Demonstrates :meth:`AmateRSClient.range_stream`,
:meth:`AmateRSClient.batch_stream`, and the new
:meth:`AmateRSClient.prefix_stream` APIs over a 100-key dataset.

Prerequisites
-------------

A running AmateRS server.  Address is read from ``AMATERS_SERVER`` or
defaults to ``http://localhost:50051``::

    python -m amaters.examples.streaming

Notes
-----

The PyO3 ``StreamIterator`` is a synchronous Python iterator returned
from an awaitable.  Iterate it with a regular ``for`` loop after
awaiting the call that produced it.
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
    print(f"[streaming] connecting to {addr}")

    async with AmateRSClient.connect(addr) as client:
        collection = "streaming_demo"

        # -- Seed 100 keys -------------------------------------------------
        items = [
            (f"item:{i:03d}".encode(), f"payload-{i:03d}".encode())
            for i in range(100)
        ]
        stored = await client.batch_set(collection, items)
        print(f"[streaming] seeded {stored} keys")

        # -- range_stream -------------------------------------------------
        stream = await client.range_stream(
            collection, b"item:000", b"item:999", chunk_size=20
        )
        total = 0
        chunks = 0
        for chunk in stream:
            chunks += 1
            total += len(chunk)
        print(f"[streaming] range_stream yielded {total} items in {chunks} chunks")
        assert total == 100

        # -- prefix_stream ------------------------------------------------
        stream = await client.prefix_stream(collection, b"item:0", chunk_size=15)
        prefix_total = 0
        for chunk in stream:
            prefix_total += len(chunk)
        # Keys item:000..item:099 (100 entries) start with "item:0".
        print(f"[streaming] prefix_stream yielded {prefix_total} matches")
        assert prefix_total == 100

        # -- batch_stream -------------------------------------------------
        get_ops = [("get", collection, k) for k, _ in items]
        bstream = await client.batch_stream(get_ops, chunk_size=25)
        retrieved = 0
        for chunk in bstream:
            for value in chunk:
                if value is not None:
                    retrieved += 1
        print(f"[streaming] batch_stream returned {retrieved} non-None values")
        assert retrieved == 100

        # -- Cleanup -------------------------------------------------------
        keys = [k for k, _ in items]
        deleted = await client.batch_delete(collection, keys)
        print(f"[streaming] cleanup removed {deleted} keys")

        print("[streaming] done.")


if __name__ == "__main__":
    asyncio.run(main())
