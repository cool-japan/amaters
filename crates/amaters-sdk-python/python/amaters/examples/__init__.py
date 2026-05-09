"""
Runnable examples for the amaters Python SDK.

Each module in this package can be executed directly, for instance::

    python -m amaters.examples.quickstart
    python -m amaters.examples.batch
    python -m amaters.examples.streaming
    python -m amaters.examples.transactions

Prerequisites
-------------

A running AmateRS server reachable at the address given by the
``AMATERS_SERVER`` environment variable.  When unset the examples
default to ``http://localhost:50051``.
"""

from __future__ import annotations

__all__: list[str] = []
