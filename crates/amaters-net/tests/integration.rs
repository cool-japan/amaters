//! Integration tests for amaters-net
//!
//! High-connection and high-rate tests require a live server and are marked #[ignore].

/// Requires 10K+ simultaneous connections to a running server.
#[test]
#[ignore = "requires live server with 10K+ connection capacity"]
fn test_high_connection_count_10k() {
    todo!("requires live server");
}

/// Requires sustained load at 100K+ rps.
#[test]
#[ignore = "requires live server capable of 100K+ rps"]
fn test_high_request_rate_100k_rps() {
    todo!("requires live server");
}
