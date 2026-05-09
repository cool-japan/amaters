"""
amaters quickstart example
==========================

End-to-end CRUD walkthrough against a live AmateRS server.

Prerequisites
-------------

A running AmateRS server.  The address is read from the
``AMATERS_SERVER`` environment variable; if unset it defaults to
``http://localhost:50051``::

    export AMATERS_SERVER=http://my-server:50051
    python -m amaters.examples.quickstart

What it does
------------

1. Connects to the server.
2. Stores three encrypted blobs under the ``"quickstart"`` collection.
3. Retrieves them with :meth:`AmateRSClient.get`.
4. Demonstrates :meth:`AmateRSClient.contains`.
5. Deletes one entry and verifies its disappearance.
6. Closes the connection cleanly via the async context-manager.
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
    print(f"[quickstart] connecting to {addr}")

    async with AmateRSClient.connect(addr) as client:
        collection = "quickstart"

        # -- Step 1: store three keys ---------------------------------------
        keys = [b"user:1", b"user:2", b"user:3"]
        values = [b"alice-ciphertext", b"bob-ciphertext", b"carol-ciphertext"]
        for key, value in zip(keys, values):
            await client.set(collection, key, value)
        print(f"[quickstart] stored {len(keys)} entries")

        # -- Step 2: read them back -----------------------------------------
        for key in keys:
            value = await client.get(collection, key)
            assert value is not None, f"expected a value for {key!r}"
            print(f"[quickstart] {key!r} -> {len(value)} bytes")

        # -- Step 3: contains -----------------------------------------------
        present = await client.contains(collection, b"user:1")
        absent = await client.contains(collection, b"user:does-not-exist")
        assert present is True
        assert absent is False
        print("[quickstart] contains check passed")

        # -- Step 4: delete and verify --------------------------------------
        await client.delete(collection, b"user:2")
        assert await client.contains(collection, b"user:2") is False
        print("[quickstart] deleted user:2 successfully")

        print("[quickstart] done.")


if __name__ == "__main__":
    asyncio.run(main())
