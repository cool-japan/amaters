"""
AmateRS Python SDK

A Python client library for AmateRS - Fully Homomorphic Encrypted Database.

Example:
    >>> import asyncio
    >>> from amaters import AmateRSClient
    >>>
    >>> async def main():
    ...     client = await AmateRSClient.connect("http://localhost:50051")
    ...     await client.set("users", b"user:123", encrypted_data)
    ...     value = await client.get("users", b"user:123")
    ...     await client.close()
    >>>
    >>> asyncio.run(main())
"""

from typing import Optional, Union, List, Tuple, Dict, Any

# Import core Rust types from the internal module
from ._internal import (
    AmateRSClient as _AmateRSClient,
    ClientConfig as _ClientConfig,
    RetryConfig as _RetryConfig,
    Key as _Key,
    __version__,
)

__all__ = [
    "AmateRSClient",
    "ClientConfig",
    "RetryConfig",
    "Key",
    "AmateRSError",
    "ConnectionError",
    "TimeoutError",
    "__version__",
]


class AmateRSError(Exception):
    """Base exception for AmateRS SDK errors."""
    pass


class ConnectionError(AmateRSError):
    """Connection-related errors."""
    pass


class TimeoutError(AmateRSError):
    """Timeout errors."""
    pass


class AmateRSClient:
    """
    AmateRS client for interacting with the database.

    This client manages connections, handles retries, and provides
    high-level operations for working with encrypted data.

    Attributes:
        _client: Internal Rust client instance

    Example:
        >>> async with AmateRSClient.connect("http://localhost:50051") as client:
        ...     await client.set("users", b"user:1", encrypted_data)
        ...     value = await client.get("users", b"user:1")
    """

    def __init__(self, _internal_client: _AmateRSClient):
        """Initialize client (internal use only)."""
        self._client = _internal_client

    @classmethod
    async def connect(cls, addr: str) -> "AmateRSClient":
        """
        Connect to an AmateRS server.

        Args:
            addr: Server address (e.g., "http://localhost:50051")

        Returns:
            Connected client instance

        Raises:
            ConnectionError: If connection fails

        Example:
            >>> client = await AmateRSClient.connect("http://localhost:50051")
        """
        try:
            internal = await _AmateRSClient.connect(addr)
            return cls(internal)
        except Exception as e:
            raise ConnectionError(f"Failed to connect: {e}") from e

    @classmethod
    async def connect_with_config(cls, config: "ClientConfig") -> "AmateRSClient":
        """
        Connect with custom configuration.

        Args:
            config: Client configuration

        Returns:
            Connected client instance

        Raises:
            ConnectionError: If connection fails

        Example:
            >>> config = ClientConfig("http://localhost:50051", connect_timeout=5)
            >>> client = await AmateRSClient.connect_with_config(config)
        """
        try:
            internal = await _AmateRSClient.connect_with_config(config._config)
            return cls(internal)
        except Exception as e:
            raise ConnectionError(f"Failed to connect: {e}") from e

    async def set(
        self,
        collection: str,
        key: Union[bytes, str],
        value: bytes,
    ) -> None:
        """
        Set a key-value pair.

        Args:
            collection: Collection name
            key: Key (bytes or str)
            value: Encrypted value (bytes)

        Raises:
            AmateRSError: If operation fails

        Example:
            >>> await client.set("users", b"user:123", encrypted_data)
            >>> await client.set("users", "user:123", encrypted_data)  # str key also works
        """
        try:
            await self._client.set(collection, key, value)
        except Exception as e:
            raise AmateRSError(f"Set operation failed: {e}") from e

    async def get(
        self,
        collection: str,
        key: Union[bytes, str],
    ) -> Optional[bytes]:
        """
        Get a value by key.

        Args:
            collection: Collection name
            key: Key (bytes or str)

        Returns:
            Encrypted value if exists, None otherwise

        Raises:
            AmateRSError: If operation fails

        Example:
            >>> value = await client.get("users", b"user:123")
            >>> if value:
            ...     print(f"Got {len(value)} bytes")
        """
        try:
            return await self._client.get(collection, key)
        except Exception as e:
            raise AmateRSError(f"Get operation failed: {e}") from e

    async def delete(
        self,
        collection: str,
        key: Union[bytes, str],
    ) -> None:
        """
        Delete a key.

        Args:
            collection: Collection name
            key: Key (bytes or str)

        Raises:
            AmateRSError: If operation fails

        Example:
            >>> await client.delete("users", b"user:123")
        """
        try:
            await self._client.delete(collection, key)
        except Exception as e:
            raise AmateRSError(f"Delete operation failed: {e}") from e

    async def contains(
        self,
        collection: str,
        key: Union[bytes, str],
    ) -> bool:
        """
        Check if a key exists.

        Args:
            collection: Collection name
            key: Key (bytes or str)

        Returns:
            True if key exists, False otherwise

        Raises:
            AmateRSError: If operation fails

        Example:
            >>> if await client.contains("users", b"user:123"):
            ...     print("Key exists")
        """
        try:
            return await self._client.contains(collection, key)
        except Exception as e:
            raise AmateRSError(f"Contains operation failed: {e}") from e

    async def batch(
        self,
        operations: List[Tuple[str, str, Union[bytes, str], Optional[bytes]]],
    ) -> List[Any]:
        """
        Execute a batch of operations.

        Args:
            operations: List of (operation, collection, key, value) tuples
                       operation can be "set", "get", or "delete"

        Returns:
            Results for each operation

        Raises:
            AmateRSError: If batch operation fails

        Example:
            >>> results = await client.batch([
            ...     ("set", "users", b"user:1", encrypted1),
            ...     ("set", "users", b"user:2", encrypted2),
            ... ])
        """
        try:
            return await self._client.batch(operations)
        except Exception as e:
            raise AmateRSError(f"Batch operation failed: {e}") from e

    async def health_check(self) -> bool:
        """
        Perform health check.

        Returns:
            True if server is healthy

        Raises:
            AmateRSError: If health check fails

        Example:
            >>> healthy = await client.health_check()
            >>> print(f"Server healthy: {healthy}")
        """
        try:
            return await self._client.health_check()
        except Exception as e:
            raise AmateRSError(f"Health check failed: {e}") from e

    def pool_stats(self) -> Dict[str, int]:
        """
        Get connection pool statistics.

        Returns:
            Dictionary with pool statistics:
            - total_connections: Total connections
            - idle_connections: Idle connections
            - active_connections: Active connections

        Example:
            >>> stats = client.pool_stats()
            >>> print(f"Active: {stats['active_connections']}")
        """
        return self._client.pool_stats()

    def close(self) -> None:
        """
        Close all connections.

        Example:
            >>> client.close()
        """
        self._client.close()

    async def __aenter__(self) -> "AmateRSClient":
        """Async context manager entry."""
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb) -> None:
        """Async context manager exit."""
        self.close()

    def __enter__(self) -> "AmateRSClient":
        """Sync context manager entry."""
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        """Sync context manager exit."""
        self.close()


class ClientConfig:
    """
    Client configuration.

    Attributes:
        server_addr: Server address
        connect_timeout: Connection timeout in seconds
        request_timeout: Request timeout in seconds
        max_connections: Maximum number of connections

    Example:
        >>> config = ClientConfig(
        ...     "http://localhost:50051",
        ...     connect_timeout=5,
        ...     request_timeout=30,
        ...     max_connections=20
        ... )
    """

    def __init__(
        self,
        server_addr: str,
        connect_timeout: int = 10,
        request_timeout: int = 30,
        max_connections: int = 10,
    ):
        """
        Create a new client configuration.

        Args:
            server_addr: Server address
            connect_timeout: Connection timeout in seconds
            request_timeout: Request timeout in seconds
            max_connections: Maximum connections
        """
        self._config = _ClientConfig(
            server_addr,
            connect_timeout,
            request_timeout,
            max_connections,
        )

    def with_retry_config(self, config: "RetryConfig") -> "ClientConfig":
        """
        Set retry configuration.

        Args:
            config: Retry configuration

        Returns:
            Self for method chaining

        Example:
            >>> retry = RetryConfig(max_retries=5)
            >>> config = ClientConfig("http://localhost:50051").with_retry_config(retry)
        """
        self._config.with_retry_config(config._config)
        return self

    @property
    def server_addr(self) -> str:
        """Get server address."""
        return self._config.server_addr

    def __repr__(self) -> str:
        """String representation."""
        return repr(self._config)


class RetryConfig:
    """
    Retry configuration.

    Attributes:
        max_retries: Maximum retry attempts
        initial_backoff_ms: Initial backoff in milliseconds

    Example:
        >>> retry = RetryConfig(max_retries=5, initial_backoff_ms=200)
    """

    def __init__(
        self,
        max_retries: int = 3,
        initial_backoff_ms: int = 100,
    ):
        """
        Create a new retry configuration.

        Args:
            max_retries: Maximum retry attempts
            initial_backoff_ms: Initial backoff in milliseconds
        """
        self._config = _RetryConfig(max_retries, initial_backoff_ms)

    @classmethod
    def no_retry(cls) -> "RetryConfig":
        """
        Create a no-retry configuration.

        Returns:
            RetryConfig with 0 retries

        Example:
            >>> config = RetryConfig.no_retry()
        """
        config = cls.__new__(cls)
        config._config = _RetryConfig.no_retry()
        return config

    def __repr__(self) -> str:
        """String representation."""
        return repr(self._config)


class Key:
    """
    Key wrapper for type safety.

    Example:
        >>> key = Key.from_str("user:123")
        >>> key = Key.from_bytes(b"user:123")
    """

    def __init__(self, _internal_key: _Key):
        """Initialize key (internal use only)."""
        self._key = _internal_key

    @classmethod
    def from_bytes(cls, data: bytes) -> "Key":
        """
        Create a Key from bytes.

        Args:
            data: Key bytes

        Returns:
            Key instance
        """
        return cls(_Key.from_bytes(data))

    @classmethod
    def from_str(cls, s: str) -> "Key":
        """
        Create a Key from string.

        Args:
            s: Key string

        Returns:
            Key instance
        """
        return cls(_Key.from_str(s))

    def to_bytes(self) -> bytes:
        """Convert to bytes."""
        return self._key.to_bytes()

    def to_string(self) -> str:
        """Convert to string (lossy)."""
        return self._key.to_string()

    def __repr__(self) -> str:
        """String representation."""
        return repr(self._key)
