# Type stubs for the amaters SDK Python bindings.
# Derived from PyO3 source in crates/amaters-sdk-python/src/
# PEP 561 compliant — see py.typed marker in this package.

from __future__ import annotations

from typing import Any, Dict, Iterator, List, Optional, Tuple, Union

__version__: str

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

# ---------------------------------------------------------------------------
# Exception hierarchy
# ---------------------------------------------------------------------------

class AmateRSError(Exception):
    """Base exception for all AmateRS SDK errors."""
    ...

class ConnectionError(AmateRSError):
    """Raised when a connection to the server cannot be established or is lost."""
    ...

class TimeoutError(AmateRSError):
    """Raised when an operation exceeds its configured timeout."""
    ...

# ---------------------------------------------------------------------------
# Key
# ---------------------------------------------------------------------------

class Key:
    """
    Opaque key wrapper providing type safety for database keys.

    Keys may be constructed from raw bytes or from a UTF-8 string.
    """

    @staticmethod
    def from_bytes(data: bytes) -> Key:
        """
        Create a Key from raw bytes.

        Args:
            data: The raw byte sequence to use as the key.

        Returns:
            A new Key wrapping the given bytes.
        """
        ...

    @staticmethod
    def from_str(s: str) -> Key:
        """
        Create a Key from a UTF-8 string.

        Args:
            s: The string to use as the key.

        Returns:
            A new Key whose byte representation is the UTF-8 encoding of *s*.
        """
        ...

    def to_bytes(self) -> bytes:
        """Return the raw byte representation of this key."""
        ...

    def to_string(self) -> str:
        """
        Return a (possibly lossy) string representation of this key.

        Non-UTF-8 bytes are replaced with the Unicode replacement character.
        """
        ...

    def __len__(self) -> int: ...
    def __eq__(self, other: object) -> bool: ...
    def __hash__(self) -> int: ...
    def __repr__(self) -> str: ...
    def __str__(self) -> str: ...

# ---------------------------------------------------------------------------
# RetryConfig
# ---------------------------------------------------------------------------

class RetryConfig:
    """
    Configuration for request retry behaviour.

    Controls how many times a failed request is retried and what
    exponential-backoff strategy is applied.

    Example::

        retry = RetryConfig(max_retries=5, initial_backoff_ms=200)
        config = ClientConfig("http://localhost:50051").with_retry_config(retry)
    """

    def __init__(
        self,
        max_retries: int = 3,
        initial_backoff_ms: int = 100,
    ) -> None:
        """
        Create a new RetryConfig.

        Args:
            max_retries: Maximum number of retry attempts (default: 3).
            initial_backoff_ms: Initial backoff delay in milliseconds (default: 100).
        """
        ...

    @staticmethod
    def no_retry() -> RetryConfig:
        """
        Create a configuration that performs no retries.

        Returns:
            A RetryConfig with max_retries=0.
        """
        ...

    @property
    def max_retries(self) -> int:
        """Maximum number of retry attempts."""
        ...

    @property
    def initial_backoff_ms(self) -> int:
        """Initial backoff delay in milliseconds."""
        ...

    def __repr__(self) -> str: ...
    def __str__(self) -> str: ...

# ---------------------------------------------------------------------------
# ClientConfig
# ---------------------------------------------------------------------------

class ClientConfig:
    """
    Configuration for the AmateRS client.

    Controls server address, connection/request timeouts, maximum
    number of concurrent connections, and optional retry policy.

    Example::

        config = ClientConfig(
            "http://localhost:50051",
            connect_timeout=5,
            request_timeout=30,
            max_connections=20,
        )
    """

    def __init__(
        self,
        server_addr: str,
        connect_timeout: int = 10,
        request_timeout: int = 30,
        max_connections: int = 10,
    ) -> None:
        """
        Create a new ClientConfig.

        Args:
            server_addr: gRPC server address, e.g. ``"http://localhost:50051"``.
            connect_timeout: Connection establishment timeout in seconds (default: 10).
            request_timeout: Per-request timeout in seconds (default: 30).
            max_connections: Maximum number of pooled connections (default: 10).
        """
        ...

    def with_retry_config(self, config: RetryConfig) -> ClientConfig:
        """
        Attach a retry configuration and return *self* for method chaining.

        Args:
            config: Retry policy to apply.

        Returns:
            This ClientConfig instance (mutated in place and returned).
        """
        ...

    @property
    def server_addr(self) -> str:
        """The gRPC server address this config targets."""
        ...

    @property
    def connect_timeout(self) -> int:
        """Connection timeout in seconds."""
        ...

    @property
    def request_timeout(self) -> int:
        """Per-request timeout in seconds."""
        ...

    @property
    def max_connections(self) -> int:
        """Maximum number of concurrent connections."""
        ...

    def __repr__(self) -> str: ...
    def __str__(self) -> str: ...

# ---------------------------------------------------------------------------
# StreamIterator
# ---------------------------------------------------------------------------

class StreamIterator:
    """
    Iterator that yields chunks of ``(key_bytes, value_bytes)`` tuples.

    Returned by :meth:`AmateRSClient.range_stream`.  Each call to
    ``__next__`` produces a *list* of up to ``chunk_size`` pairs.

    Example::

        stream = await client.range_stream("users", "a", "z", chunk_size=50)
        for chunk in stream:
            for key, value in chunk:
                process(key, value)
    """

    def __iter__(self) -> Iterator[List[Tuple[bytes, bytes]]]: ...
    def __next__(self) -> Optional[List[Tuple[bytes, bytes]]]: ...
    def __len__(self) -> int: ...
    def __repr__(self) -> str: ...

    def collect(self) -> List[Tuple[bytes, bytes]]:
        """
        Consume and return all remaining items as a flat list.

        After this call ``remaining`` will be 0.

        Returns:
            All remaining ``(key_bytes, value_bytes)`` tuples.
        """
        ...

    @property
    def remaining(self) -> int:
        """Number of items that have not yet been yielded."""
        ...

    @property
    def chunk_size(self) -> int:
        """Number of items returned per chunk."""
        ...

# ---------------------------------------------------------------------------
# BatchStreamIterator
# ---------------------------------------------------------------------------

class BatchStreamIterator:
    """
    Iterator that yields chunks of batch-operation results.

    Returned by :meth:`AmateRSClient.batch_stream`.  Each chunk is a
    list whose elements are ``bytes | None | int`` depending on the
    corresponding operation type.

    Example::

        stream = await client.batch_stream(operations, chunk_size=25)
        for chunk in stream:
            for result in chunk:
                handle(result)
    """

    def __iter__(self) -> Iterator[List[Any]]: ...
    def __next__(self) -> Optional[List[Any]]: ...
    def __len__(self) -> int: ...
    def __repr__(self) -> str: ...

    def collect(self) -> List[Any]:
        """
        Consume and return all remaining results as a flat list.

        Returns:
            All remaining result objects.
        """
        ...

    @property
    def remaining(self) -> int:
        """Number of results that have not yet been yielded."""
        ...

# ---------------------------------------------------------------------------
# BatchResult
# ---------------------------------------------------------------------------

class BatchResult:
    """
    Iterable container holding the results of a batch operation.

    Each element is ``bytes`` for a successful get, ``None`` for a
    set/delete or a key-not-found result, or ``int`` for an
    affected-rows count.

    Example::

        result = BatchResult(...)
        for item in result:
            if item is not None:
                print(f"Got value of length {len(item)}")
    """

    def __iter__(self) -> Iterator[Optional[Union[bytes, int]]]: ...
    def __next__(self) -> Optional[Union[bytes, int]]: ...
    def __len__(self) -> int: ...
    def __getitem__(self, index: int) -> Optional[Union[bytes, int]]: ...
    def __repr__(self) -> str: ...

# ---------------------------------------------------------------------------
# ScanResult
# ---------------------------------------------------------------------------

class ScanResult:
    """
    Single page of results from a cursor-based scan operation.

    Returned by :meth:`AmateRSClient.scan`.  The ``next_cursor`` field
    should be passed to the subsequent :meth:`~AmateRSClient.scan` call
    to retrieve the next page.

    Example::

        result = await client.scan("users", "user:", limit=50)
        while result.has_more:
            process(result.results)
            result = await client.scan(
                "users", "user:",
                cursor=result.next_cursor,
                limit=50,
            )
        process(result.results)  # final page
    """

    @property
    def results(self) -> List[Tuple[bytes, bytes]]:
        """Key-value pairs on this page as ``(key_bytes, value_bytes)`` tuples."""
        ...

    @property
    def next_cursor(self) -> Optional[str]:
        """
        Opaque cursor string for the next page, or ``None`` if this is
        the last page.
        """
        ...

    @property
    def has_more(self) -> bool:
        """``True`` if there are additional pages after this one."""
        ...

    def __len__(self) -> int: ...
    def __repr__(self) -> str: ...

# ---------------------------------------------------------------------------
# AmateRSClient
# ---------------------------------------------------------------------------

class AmateRSClient:
    """
    Client for the AmateRS Fully Homomorphic Encrypted database.

    All I/O methods are **coroutines** and must be awaited.  The client
    implements both the sync and async context-manager protocols.

    Typical usage::

        async with AmateRSClient.connect("http://localhost:50051") as client:
            await client.set("users", b"user:1", encrypted_data)
            value = await client.get("users", b"user:1")
    """

    # ------------------------------------------------------------------
    # Construction
    # ------------------------------------------------------------------

    @staticmethod
    async def connect(addr: str) -> AmateRSClient:
        """
        Connect to an AmateRS server.

        Args:
            addr: gRPC server address, e.g. ``"http://localhost:50051"``.

        Returns:
            A connected :class:`AmateRSClient` instance.

        Raises:
            ConnectionError: If the server cannot be reached.

        Example::

            client = await AmateRSClient.connect("http://localhost:50051")
        """
        ...

    @staticmethod
    async def connect_with_config(config: ClientConfig) -> AmateRSClient:
        """
        Connect using a custom :class:`ClientConfig`.

        Args:
            config: Client configuration (timeouts, connection pool, retry).

        Returns:
            A connected :class:`AmateRSClient` instance.

        Raises:
            ConnectionError: If the server cannot be reached.

        Example::

            cfg = ClientConfig("http://localhost:50051", connect_timeout=5)
            client = await AmateRSClient.connect_with_config(cfg)
        """
        ...

    # ------------------------------------------------------------------
    # Core CRUD
    # ------------------------------------------------------------------

    async def set(
        self,
        collection: str,
        key: Union[bytes, str],
        value: bytes,
    ) -> None:
        """
        Store a key-value pair.

        Args:
            collection: Name of the collection to write into.
            key: Key — either raw bytes or a UTF-8 string.
            value: Encrypted value bytes to store.

        Raises:
            AmateRSError: If the operation fails.

        Example::

            await client.set("users", b"user:123", encrypted_data)
            await client.set("users", "user:123", encrypted_data)  # str also works
        """
        ...

    async def get(
        self,
        collection: str,
        key: Union[bytes, str],
    ) -> Optional[bytes]:
        """
        Retrieve a value by key.

        Args:
            collection: Collection to read from.
            key: Key to look up.

        Returns:
            The stored bytes, or ``None`` if the key does not exist.

        Raises:
            AmateRSError: If the operation fails.

        Example::

            value = await client.get("users", b"user:123")
            if value is not None:
                print(f"Got {len(value)} bytes")
        """
        ...

    async def delete(
        self,
        collection: str,
        key: Union[bytes, str],
    ) -> None:
        """
        Delete a key.

        Args:
            collection: Collection containing the key.
            key: Key to delete.

        Raises:
            AmateRSError: If the operation fails.

        Example::

            await client.delete("users", b"user:123")
        """
        ...

    async def contains(
        self,
        collection: str,
        key: Union[bytes, str],
    ) -> bool:
        """
        Check whether a key exists.

        Args:
            collection: Collection to check.
            key: Key to test.

        Returns:
            ``True`` if the key exists, ``False`` otherwise.

        Raises:
            AmateRSError: If the operation fails.

        Example::

            if await client.contains("users", b"user:123"):
                print("Key exists")
        """
        ...

    # ------------------------------------------------------------------
    # Batch operations
    # ------------------------------------------------------------------

    async def batch(
        self,
        operations: List[Tuple[str, str, Union[bytes, str], Optional[bytes]]],
    ) -> List[Any]:
        """
        Execute a heterogeneous batch of operations atomically.

        Each element of *operations* must be one of:

        * ``("set", collection, key, value)`` — store *value* at *key*.
        * ``("get", collection, key)`` — retrieve the value at *key*.
        * ``("delete", collection, key)`` — remove *key*.

        Args:
            operations: List of operation tuples as described above.

        Returns:
            A list of results, one per operation.  ``set``/``delete``
            results are ``None``; ``get`` results are ``bytes`` or ``None``.

        Raises:
            AmateRSError: If the batch operation fails.

        Example::

            results = await client.batch([
                ("set",    "users", b"user:1", encrypted1),
                ("get",    "users", b"user:1"),
                ("delete", "users", b"user:2"),
            ])
        """
        ...

    async def batch_set(
        self,
        collection: str,
        items: List[Tuple[Union[bytes, str], bytes]],
    ) -> int:
        """
        Store multiple key-value pairs in a single batch.

        Args:
            collection: Target collection.
            items: List of ``(key, value)`` tuples.

        Returns:
            Number of items successfully stored.

        Raises:
            AmateRSError: If the operation fails.

        Example::

            count = await client.batch_set("users", [
                (b"user:1", encrypted1),
                (b"user:2", encrypted2),
            ])
        """
        ...

    async def batch_get(
        self,
        collection: str,
        keys: List[Union[bytes, str]],
    ) -> List[Tuple[bytes, Optional[bytes]]]:
        """
        Retrieve multiple values in a single batch.

        Args:
            collection: Source collection.
            keys: Keys to look up.

        Returns:
            List of ``(key_bytes, value_or_none)`` tuples, preserving
            the order of *keys*.

        Raises:
            AmateRSError: If the operation fails.

        Example::

            results = await client.batch_get("users", [b"user:1", b"user:2"])
            for key, value in results:
                if value is not None:
                    print(f"Found {key!r}")
        """
        ...

    async def batch_delete(
        self,
        collection: str,
        keys: List[Union[bytes, str]],
    ) -> int:
        """
        Delete multiple keys in a single batch.

        Args:
            collection: Collection containing the keys.
            keys: Keys to delete.

        Returns:
            Number of delete operations executed.

        Raises:
            AmateRSError: If the operation fails.

        Example::

            deleted = await client.batch_delete("users", [b"user:1", b"user:2"])
        """
        ...

    # ------------------------------------------------------------------
    # Range / scan operations
    # ------------------------------------------------------------------

    async def range_query(
        self,
        collection: str,
        start: Union[bytes, str],
        end: Union[bytes, str],
    ) -> List[Tuple[bytes, bytes]]:
        """
        Retrieve all key-value pairs whose keys fall within [*start*, *end*].

        Both *start* and *end* are inclusive.

        Args:
            collection: Collection to query.
            start: Lower bound key (inclusive).
            end: Upper bound key (inclusive).

        Returns:
            List of ``(key_bytes, value_bytes)`` tuples ordered by key.

        Raises:
            AmateRSError: If the query fails.

        Example::

            results = await client.range_query("users", "user:000", "user:999")
            for key, value in results:
                print(f"Key: {key!r}, value length: {len(value)}")
        """
        ...

    async def count(
        self,
        collection: str,
        start: Union[bytes, str],
        end: Union[bytes, str],
    ) -> int:
        """
        Return the number of keys in the range [*start*, *end*].

        Args:
            collection: Collection to query.
            start: Lower bound key (inclusive).
            end: Upper bound key (inclusive).

        Returns:
            Number of key-value pairs in the specified range.

        Raises:
            AmateRSError: If the query fails.

        Example::

            n = await client.count("users", "user:000", "user:999")
        """
        ...

    async def keys(
        self,
        collection: str,
        start: Union[bytes, str],
        end: Union[bytes, str],
    ) -> List[bytes]:
        """
        Return all keys in the range [*start*, *end*] without their values.

        Args:
            collection: Collection to query.
            start: Lower bound key (inclusive).
            end: Upper bound key (inclusive).

        Returns:
            List of key byte strings, ordered ascending.

        Raises:
            AmateRSError: If the query fails.

        Example::

            keys = await client.keys("users", "user:000", "user:999")
        """
        ...

    async def range_stream(
        self,
        collection: str,
        start: Union[bytes, str],
        end: Union[bytes, str],
        chunk_size: int = 100,
    ) -> StreamIterator:
        """
        Stream range-query results in chunks via a :class:`StreamIterator`.

        Use this instead of :meth:`range_query` when the result set may
        be large and should be processed incrementally.

        Args:
            collection: Collection to query.
            start: Lower bound key (inclusive).
            end: Upper bound key (inclusive).
            chunk_size: Number of ``(key, value)`` pairs per chunk (default: 100).
                        Values ``<= 0`` are clamped to 1.

        Returns:
            A :class:`StreamIterator` that yields lists of
            ``(key_bytes, value_bytes)`` tuples.

        Raises:
            AmateRSError: If the query fails.

        Example::

            stream = await client.range_stream("users", "a", "z", chunk_size=50)
            for chunk in stream:
                for key, value in chunk:
                    process(key, value)
        """
        ...

    async def prefix_query(
        self,
        collection: str,
        prefix: Union[bytes, str],
    ) -> List[Tuple[bytes, bytes]]:
        """
        Retrieve all key-value pairs whose keys begin with *prefix*.

        Equivalent to a half-open range query
        ``[prefix, prefix_upper_bound(prefix))``.  When *prefix* is empty
        or consists entirely of ``0xFF`` bytes the upper bound is open and
        a 256-byte ``0xFF`` sentinel is used internally so the call still
        returns every matching key.

        Args:
            collection: Collection to query.
            prefix: Key prefix — only keys beginning with this byte
                    sequence (or its UTF-8 encoding when *prefix* is a
                    string) are returned.

        Returns:
            List of ``(key_bytes, value_bytes)`` tuples ordered by key.

        Raises:
            AmateRSError: If the query fails.

        Example::

            rows = await client.prefix_query("users", b"user:")
            for key, value in rows:
                print(f"key={key!r}, value_len={len(value)}")
        """
        ...

    async def prefix_stream(
        self,
        collection: str,
        prefix: Union[bytes, str],
        chunk_size: int = 100,
    ) -> StreamIterator:
        """
        Stream prefix-query results in chunks via a :class:`StreamIterator`.

        Use this instead of :meth:`prefix_query` when the matching set may
        be large and should be processed incrementally.

        Args:
            collection: Collection to query.
            prefix: Key prefix.
            chunk_size: Number of ``(key, value)`` pairs per chunk
                        (default: 100).  Values ``<= 0`` are clamped to 1.

        Returns:
            A :class:`StreamIterator` that yields lists of
            ``(key_bytes, value_bytes)`` tuples whose keys share *prefix*.

        Raises:
            AmateRSError: If the query fails.

        Example::

            stream = await client.prefix_stream("users", b"user:", chunk_size=50)
            for chunk in stream:
                for key, value in chunk:
                    process(key, value)
        """
        ...

    async def batch_stream(
        self,
        operations: List[Tuple[str, str, Union[bytes, str], Optional[bytes]]],
        chunk_size: int = 50,
    ) -> BatchStreamIterator:
        """
        Execute a batch and stream results in chunks via a :class:`BatchStreamIterator`.

        The *operations* format is identical to :meth:`batch`.

        Args:
            operations: List of operation tuples.
            chunk_size: Number of results per chunk (default: 50).
                        Values ``<= 0`` are clamped to 1.

        Returns:
            A :class:`BatchStreamIterator` that yields lists of results.

        Raises:
            AmateRSError: If the batch operation fails.

        Example::

            stream = await client.batch_stream(operations, chunk_size=25)
            for chunk in stream:
                for result in chunk:
                    handle(result)
        """
        ...

    async def scan(
        self,
        collection: str,
        prefix: Union[bytes, str],
        cursor: Optional[str] = None,
        limit: int = 100,
    ) -> ScanResult:
        """
        Cursor-based paginated scan for keys sharing a common prefix.

        On the first call omit *cursor* (or pass ``None``).  Subsequent
        calls should pass ``result.next_cursor`` until ``result.has_more``
        is ``False``.

        Args:
            collection: Collection to scan.
            prefix: Key prefix — only keys beginning with this value are returned.
            cursor: Opaque cursor from a previous :meth:`scan` result, or ``None``
                    for the first page.
            limit: Maximum number of items to return per page (default: 100).
                   Values ``<= 0`` are clamped to 1.

        Returns:
            A :class:`ScanResult` containing results for this page, a
            ``next_cursor`` for the following page, and a ``has_more`` flag.

        Raises:
            AmateRSError: If the scan fails.
            ValueError: If *cursor* is not a valid cursor string.

        Example::

            result = await client.scan("users", "user:", limit=50)
            while result.has_more:
                process(result.results)
                result = await client.scan(
                    "users", "user:",
                    cursor=result.next_cursor,
                    limit=50,
                )
            process(result.results)
        """
        ...

    # ------------------------------------------------------------------
    # Utility / diagnostics
    # ------------------------------------------------------------------

    async def health_check(self) -> bool:
        """
        Verify that the server is reachable and healthy.

        Returns:
            ``True`` if the server responds successfully.

        Raises:
            AmateRSError: If the health check fails or the server is unreachable.

        Example::

            healthy = await client.health_check()
        """
        ...

    def pool_stats(self) -> Dict[str, int]:
        """
        Return connection-pool statistics.

        Returns:
            A dictionary with the following integer keys:

            * ``"total_connections"`` — total connections in the pool.
            * ``"idle_connections"`` — connections currently idle.
            * ``"active_connections"`` — connections currently in use.

        Example::

            stats = client.pool_stats()
            print(f"Active: {stats['active_connections']}")
        """
        ...

    def close(self) -> None:
        """
        Close all pooled connections.

        Called automatically when the client is used as a context manager.

        Example::

            client.close()
        """
        ...

    # ------------------------------------------------------------------
    # Context-manager protocol (sync)
    # ------------------------------------------------------------------

    def __enter__(self) -> AmateRSClient: ...
    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None: ...

    # ------------------------------------------------------------------
    # Async context-manager protocol
    # ------------------------------------------------------------------

    async def __aenter__(self) -> AmateRSClient: ...
    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None: ...

    # ------------------------------------------------------------------
    # Dunder helpers
    # ------------------------------------------------------------------

    def __repr__(self) -> str: ...
    def __str__(self) -> str: ...
