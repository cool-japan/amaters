/**
 * AmateRS TypeScript SDK - Node.js Example
 *
 * This example demonstrates how to use the AmateRS SDK in a Node.js environment.
 * It shows basic CRUD operations and query building.
 *
 * Run with: npx ts-node examples/node-example.ts
 */

// Note: In production, you would import from '@amaters/sdk'
// For local development, import from the built package
import type {
  AmateRSClient,
  Key,
  CipherBlob,
  ClientConfig,
  QueryBuilder,
  ColumnRef,
} from '../src/ts/index';

// Placeholder for WASM imports - replace with actual imports after build
// import init, {
//   AmateRSClient,
//   Key,
//   CipherBlob,
//   ClientConfig,
//   QueryBuilder,
//   col,
// } from '../pkg/node/amaters_sdk';

/**
 * Example: Basic CRUD Operations
 */
async function basicCrudExample(client: AmateRSClient): Promise<void> {
  console.log('\n=== Basic CRUD Operations ===\n');

  // Placeholder types for demonstration
  const Key = {
    fromString: (s: string): Key => ({ toBytes: () => new Uint8Array(), toString: () => s, length: s.length, isEmpty: () => false, equals: () => true, compareTo: () => 0 }),
    fromBytes: (b: Uint8Array): Key => ({ toBytes: () => b, toString: () => '', length: b.length, isEmpty: () => false, equals: () => true, compareTo: () => 0 }),
    maxSize: 65536,
  };

  const CipherBlob = {
    fromBytes: (b: Uint8Array): CipherBlob => ({ toBytes: () => b, length: b.length, isEmpty: () => false, verifyIntegrity: () => true, equals: () => true }),
    maxSize: 1073741824,
  };

  // Create keys and values
  const userKey = Key.fromString('user:123');
  const userData = new TextEncoder().encode('{"name": "Alice", "email": "alice@example.com"}');
  const userBlob = CipherBlob.fromBytes(userData);

  // Set a value
  console.log('Setting user data...');
  await client.set('users', userKey, userBlob);
  console.log('  User data set successfully');

  // Get a value
  console.log('Getting user data...');
  const retrieved = await client.get('users', userKey);
  if (retrieved) {
    console.log(`  Retrieved ${retrieved.length} bytes`);
    const decoded = new TextDecoder().decode(retrieved.toBytes());
    console.log(`  Data: ${decoded}`);
  } else {
    console.log('  User not found');
  }

  // Check if key exists
  console.log('Checking if key exists...');
  const exists = await client.contains('users', userKey);
  console.log(`  Key exists: ${exists}`);

  // Delete the key
  console.log('Deleting user data...');
  await client.delete('users', userKey);
  console.log('  User data deleted');

  // Verify deletion
  const afterDelete = await client.get('users', userKey);
  console.log(`  After deletion: ${afterDelete ? 'still exists' : 'deleted'}`);
}

/**
 * Example: Batch Operations
 */
async function batchOperationsExample(client: AmateRSClient): Promise<void> {
  console.log('\n=== Batch Operations ===\n');

  const Key = {
    fromString: (s: string): Key => ({ toBytes: () => new Uint8Array(), toString: () => s, length: s.length, isEmpty: () => false, equals: () => true, compareTo: () => 0 }),
  };

  const CipherBlob = {
    fromBytes: (b: Uint8Array): CipherBlob => ({ toBytes: () => b, length: b.length, isEmpty: () => false, verifyIntegrity: () => true, equals: () => true }),
  };

  // Prepare batch operations
  const operations = [
    {
      type: 'set',
      collection: 'products',
      key: Key.fromString('product:1'),
      value: CipherBlob.fromBytes(new TextEncoder().encode('{"name": "Widget A", "price": 99}')),
    },
    {
      type: 'set',
      collection: 'products',
      key: Key.fromString('product:2'),
      value: CipherBlob.fromBytes(new TextEncoder().encode('{"name": "Widget B", "price": 149}')),
    },
    {
      type: 'set',
      collection: 'products',
      key: Key.fromString('product:3'),
      value: CipherBlob.fromBytes(new TextEncoder().encode('{"name": "Widget C", "price": 199}')),
    },
  ] as unknown[];

  console.log('Executing batch of 3 set operations...');
  const results = await client.batch(operations as never[]);
  console.log(`  Batch completed with ${results.length} results`);
}

/**
 * Example: Range Queries
 */
async function rangeQueryExample(client: AmateRSClient): Promise<void> {
  console.log('\n=== Range Queries ===\n');

  const Key = {
    fromString: (s: string): Key => ({ toBytes: () => new Uint8Array(), toString: () => s, length: s.length, isEmpty: () => false, equals: () => true, compareTo: () => 0 }),
  };

  // Query products in range
  const startKey = Key.fromString('product:1');
  const endKey = Key.fromString('product:9');

  console.log('Executing range query...');
  const items = await client.range('products', startKey, endKey);
  console.log(`  Found ${items.length} items in range:`);

  for (const item of items) {
    const keyStr = item.key.toString();
    const valueBytes = item.value.toBytes();
    console.log(`    ${keyStr}: ${valueBytes.length} bytes`);
  }
}

/**
 * Example: Query Builder (Fluent API)
 */
async function queryBuilderExample(client: AmateRSClient): Promise<void> {
  console.log('\n=== Query Builder (Fluent API) ===\n');

  // Note: These would be actual WASM imports in production
  console.log('Query builder examples (conceptual):');

  console.log(`
  // Simple get query
  const getQuery = new QueryBuilder('users')
    .get(Key.fromString('user:123'));

  // Set query
  const setQuery = new QueryBuilder('users')
    .set(Key.fromString('user:123'), cipherBlob);

  // Filter query with predicate
  const filterQuery = new QueryBuilder('users')
    .whereClause()
    .eq(col('status'), statusBlob)
    .and(Predicate.gt(col('age'), ageBlob))
    .build();

  // Range query
  const rangeQuery = new QueryBuilder('data')
    .range(Key.fromString('a'), Key.fromString('z'));

  // Update query
  const updateQuery = new QueryBuilder('users')
    .whereClause()
    .eq(col('id'), idBlob)
    .update([UpdateOp.set(col('status'), newStatusBlob)]);
  `);
}

/**
 * Example: Configuration Options
 */
async function configurationExample(): Promise<void> {
  console.log('\n=== Configuration Options ===\n');

  console.log(`
  // Create custom configuration
  const config = new ClientConfig('http://localhost:50051')
    .withConnectTimeout(5000)      // 5 second connection timeout
    .withRequestTimeout(30000)     // 30 second request timeout
    .withMaxConnections(20)        // Up to 20 connections
    .withMaxRetries(5)             // Retry failed operations up to 5 times
    .withInitialBackoff(100);      // Start with 100ms backoff

  // Connect with custom config
  const client = await AmateRSClient.connectWithConfig(config);

  // Check connection pool stats
  const stats = client.poolStats;
  console.log('Total connections:', stats.totalConnections);
  console.log('Idle connections:', stats.idleConnections);
  console.log('Active connections:', stats.activeConnections);
  `);
}

/**
 * Example: Error Handling
 */
async function errorHandlingExample(): Promise<void> {
  console.log('\n=== Error Handling ===\n');

  console.log(`
  try {
    const client = await AmateRSClient.connect('http://invalid-server:50051');
  } catch (error) {
    if (error instanceof AmateRSError) {
      console.log('Error code:', error.codeString());
      console.log('Message:', error.message);
      console.log('Retryable:', error.retryable);

      if (error.code === ErrorCode.Connection) {
        console.log('Connection failed, please check server address');
      } else if (error.code === ErrorCode.Timeout) {
        console.log('Operation timed out, please try again');
      }
    }
  }
  `);
}

/**
 * Example: Health Checks and Reconnection
 */
async function healthCheckExample(client: AmateRSClient): Promise<void> {
  console.log('\n=== Health Checks ===\n');

  // Perform health check
  console.log('Performing health check...');
  const healthy = await client.healthCheck();
  console.log(`  Server healthy: ${healthy}`);

  // Check connection status
  console.log(`  Client connected: ${client.isConnected}`);

  // Close and reconnect
  console.log('Closing connection...');
  client.close();
  console.log(`  Client connected after close: ${client.isConnected}`);

  console.log('Reconnecting...');
  await client.reconnect();
  console.log(`  Client connected after reconnect: ${client.isConnected}`);
}

/**
 * Main entry point
 */
async function main(): Promise<void> {
  console.log('============================================');
  console.log('  AmateRS TypeScript SDK - Node.js Example');
  console.log('============================================');

  // Note: In production, you would initialize and connect like this:
  // await init(); // Initialize WASM module
  // const client = await AmateRSClient.connect('http://localhost:50051');

  // For this demo, we'll show the API structure
  console.log('\nNote: This example shows the API structure.');
  console.log('In production, run after building the WASM module.');

  // Show configuration example (no actual connection needed)
  await configurationExample();

  // Show error handling patterns
  await errorHandlingExample();

  console.log('\n============================================');
  console.log('  Example complete!');
  console.log('============================================');

  // In production with actual WASM module:
  // try {
  //   await init();
  //   const client = await AmateRSClient.connect('http://localhost:50051');
  //
  //   await basicCrudExample(client);
  //   await batchOperationsExample(client);
  //   await rangeQueryExample(client);
  //   await queryBuilderExample(client);
  //   await healthCheckExample(client);
  //
  //   client.close();
  // } catch (error) {
  //   console.error('Error:', error);
  //   process.exit(1);
  // }
}

// Run main
main().catch(console.error);
