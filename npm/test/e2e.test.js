'use strict';

const { test, describe, before, after } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const net = require('node:net');
const { spawnSync } = require('node:child_process');

const {
  getBinaryName,
  resolveBinaryPath,
  spawnForgeServer,
  ForgeMcpClient
} = require('../index.js');
const { getCachedBinaryPath } = require('../lib/platform.js');
const PACKAGE_VERSION = require('../package.json').version;

/**
 * Allocates an available local TCP port on 127.0.0.1.
 * @returns {Promise<number>}
 */
function getAvailablePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.on('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const port = typeof address === 'object' && address ? address.port : 0;
      server.close(() => resolve(port));
    });
  });
}

/**
 * Creates a temporary multi-language workspace fixture for E2E verification.
 * @returns {string} Path to temporary workspace directory
 */
function createFixtureWorkspace() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'forge-mcp-npm-e2e-'));
  const srcDir = path.join(dir, 'src');
  const testsDir = path.join(dir, 'tests');
  fs.mkdirSync(srcDir, { recursive: true });
  fs.mkdirSync(testsDir, { recursive: true });

  fs.writeFileSync(
    path.join(srcDir, 'auth.rs'),
    [
      '/// Hashes a raw secret token using a deterministic salt.',
      'pub fn hash_secret(secret: &str) -> String {',
      '    format!("hashed:{}", secret)',
      '}',
      '',
      '/// Verifies an incoming bearer token against the expected hash.',
      'pub fn verify_token(token: &str, expected: &str) -> bool {',
      '    let computed = hash_secret(token);',
      '    computed == expected',
      '}',
      '',
      'pub struct TokenValidator {',
      '    pub issuer: String,',
      '}',
      '',
      'impl TokenValidator {',
      '    pub fn check(&self, token: &str, expected: &str) -> bool {',
      '        verify_token(token, expected)',
      '    }',
      '}',
      ''
    ].join('\n'),
    'utf8'
  );

  fs.writeFileSync(
    path.join(srcDir, 'service.ts'),
    [
      'export interface AuthRequest {',
      '  userId: string;',
      '  token: string;',
      '}',
      '',
      'export function validateSessionToken(token: string): boolean {',
      '  return token.startsWith("bearer_");',
      '}',
      '',
      'export class AuthService {',
      '  authenticateUser(req: AuthRequest): boolean {',
      '    return validateSessionToken(req.token);',
      '  }',
      '}',
      ''
    ].join('\n'),
    'utf8'
  );

  fs.writeFileSync(
    path.join(srcDir, 'worker.py'),
    [
      'def compute_digest(payload: str) -> str:',
      '    return f"digest:{payload}"',
      '',
      'def process_job(job_id: str, payload: str) -> dict:',
      '    digest = compute_digest(payload)',
      '    return {"job_id": job_id, "digest": digest}',
      ''
    ].join('\n'),
    'utf8'
  );

  fs.writeFileSync(
    path.join(srcDir, 'handler.go'),
    [
      'package main',
      '',
      'func FormatResponse(status int, body string) string {',
      '    return body',
      '}',
      '',
      'func HandleRequest(path string) string {',
      '    return FormatResponse(200, path)',
      '}',
      ''
    ].join('\n'),
    'utf8'
  );

  fs.writeFileSync(
    path.join(testsDir, 'auth_test.rs'),
    [
      'use crate::auth::{verify_token, hash_secret};',
      '',
      '#[test]',
      'fn test_verify_token_valid() {',
      '    let expected = hash_secret("secret123");',
      '    assert!(verify_token("secret123", &expected));',
      '}',
      ''
    ].join('\n'),
    'utf8'
  );

  return dir;
}

/**
 * Builds tool arguments tailored to the server's advertised JSON Schema for a tool,
 * falling back to standard field names when schema properties are open.
 *
 * @param {Map<string, any>} toolMap
 * @param {string} toolName
 * @param {Record<string, any>} candidateArgs
 * @returns {Record<string, any>}
 */
function buildToolArgs(toolMap, toolName, candidateArgs) {
  const toolDef = toolMap.get(toolName);
  const props = toolDef && toolDef.inputSchema && toolDef.inputSchema.properties;
  if (!props || typeof props !== 'object' || Object.keys(props).length === 0) {
    return candidateArgs;
  }

  const filtered = {};
  for (const [key, propSchema] of Object.entries(props)) {
    if (candidateArgs[key] !== undefined) {
      if (
        propSchema &&
        Array.isArray(propSchema.enum) &&
        propSchema.enum.length > 0 &&
        !propSchema.enum.includes(candidateArgs[key])
      ) {
        filtered[key] = propSchema.enum[0];
      } else {
        filtered[key] = candidateArgs[key];
      }
    }
  }

  return Object.keys(filtered).length > 0 ? filtered : candidateArgs;
}

/**
 * Validates that a tool response conforms to the Forge MCP envelope contract.
 * @param {any} response
 * @param {string} toolName
 */
function assertValidEnvelope(response, toolName) {
  assert.ok(response && typeof response === 'object', `${toolName} should return an object`);
  const env = response.envelope || response;
  assert.ok(
    typeof env.snapshot_id === 'string' || env.snapshot_id === null || env.results !== undefined,
    `${toolName} should include snapshot_id or results in envelope`
  );
  if (env.freshness !== undefined) {
    assert.ok(
      typeof env.freshness === 'string',
      `${toolName} freshness should be a string when present`
    );
  }
  assert.ok(
    env.results !== undefined,
    `${toolName} envelope should contain a "results" field`
  );
  assert.ok(
    Array.isArray(env.evidence),
    `${toolName} envelope should contain an "evidence" array`
  );
  assert.ok(
    Array.isArray(env.limitations),
    `${toolName} envelope should contain a "limitations" array`
  );
  assert.equal(
    typeof env.truncated,
    'boolean',
    `${toolName} envelope should contain boolean "truncated"`
  );
}

/**
 * Exercises the complete Forge MCP tool suite through a connected ForgeMcpClient.
 *
 * @param {ForgeMcpClient} client
 * @param {string} fixtureDir
 */
async function exerciseFullToolSuite(client, fixtureDir) {
  const initResult = await client.initialize();
  assert.ok(initResult, 'initialize() should return server capabilities and info');

  const toolsResponse = await client.listTools();
  assert.ok(
    toolsResponse && Array.isArray(toolsResponse.tools),
    'listTools() should return a tools array'
  );

  const toolMap = new Map(toolsResponse.tools.map((t) => [t.name, t]));
  const requiredTools = [
    'workspace_open',
    'workspace_status',
    'code_search',
    'symbol_find',
    'symbol_read',
    'symbol_references',
    'graph_query',
    'impact_analyze',
    'context_build',
    'job_status'
  ];

  for (const name of requiredTools) {
    assert.ok(toolMap.has(name), `Server should advertise required tool "${name}"`);
  }

  // 1. workspace_open
  const openRes = await client.callTool(
    'workspace_open',
    buildToolArgs(toolMap, 'workspace_open', {
      path: fixtureDir,
      workspace_path: fixtureDir,
      root_path: fixtureDir,
      workspace_id: 'e2e-workspace'
    })
  );
  assertValidEnvelope(openRes, 'workspace_open');

  const openResults = (openRes.envelope || openRes).results || {};
  const workspaceId =
    openResults.workspace_id ||
    (Array.isArray(openResults) && openResults[0] && openResults[0].workspace_id) ||
    ((openRes.envelope || openRes).evidence &&
      (openRes.envelope || openRes).evidence[0] &&
      (openRes.envelope || openRes).evidence[0].workspace_id) ||
    'e2e-workspace';
  const jobId =
    openResults.job_id ||
    (Array.isArray(openResults) && openResults[0] && openResults[0].job_id) ||
    null;

  // 2. workspace_status
  const statusRes = await client.callTool(
    'workspace_status',
    buildToolArgs(toolMap, 'workspace_status', {
      workspace_id: workspaceId,
      path: fixtureDir,
      workspace_path: fixtureDir
    })
  );
  assertValidEnvelope(statusRes, 'workspace_status');
  assert.equal((statusRes.envelope || statusRes).freshness, 'current');

  // 3. code_search
  const searchRes = await client.callTool(
    'code_search',
    buildToolArgs(toolMap, 'code_search', {
      workspace_id: workspaceId,
      path: fixtureDir,
      query: 'verify_token hash_secret',
      limit: 10,
      top_k: 10
    })
  );
  assertValidEnvelope(searchRes, 'code_search');

  // 4. symbol_find
  const findRes = await client.callTool(
    'symbol_find',
    buildToolArgs(toolMap, 'symbol_find', {
      workspace_id: workspaceId,
      path: fixtureDir,
      name: 'verify_token',
      query: 'verify_token',
      symbol: 'verify_token',
      pattern: 'verify_token'
    })
  );
  assertValidEnvelope(findRes, 'symbol_find');

  const findResults = (findRes.envelope || findRes).results;
  const firstSymbol = Array.isArray(findResults)
    ? findResults[0]
    : findResults && Array.isArray(findResults.symbols)
      ? findResults.symbols[0]
      : null;
  const symbolId = firstSymbol ? firstSymbol.symbol_id || firstSymbol.id : undefined;

  // 5. symbol_read
  const readRes = await client.callTool(
    'symbol_read',
    buildToolArgs(toolMap, 'symbol_read', {
      workspace_id: workspaceId,
      path: fixtureDir,
      symbol_id: symbolId,
      id: symbolId,
      name: 'verify_token',
      symbol: 'verify_token',
      file_path: 'src/auth.rs'
    })
  );
  assertValidEnvelope(readRes, 'symbol_read');

  // 6. symbol_references
  const refsRes = await client.callTool(
    'symbol_references',
    buildToolArgs(toolMap, 'symbol_references', {
      workspace_id: workspaceId,
      path: fixtureDir,
      symbol_id: symbolId,
      id: symbolId,
      name: 'verify_token',
      symbol: 'verify_token',
      file_path: 'src/auth.rs'
    })
  );
  assertValidEnvelope(refsRes, 'symbol_references');

  // 7. graph_query
  const graphRes = await client.callTool(
    'graph_query',
    buildToolArgs(toolMap, 'graph_query', {
      workspace_id: workspaceId,
      path: fixtureDir,
      symbol_id: symbolId,
      id: symbolId,
      symbol: 'verify_token',
      name: 'verify_token',
      direction: 'callees',
      kind: 'callees',
      relation: 'callees',
      query_type: 'callees',
      operation: 'callees',
      max_depth: 2,
      depth: 2
    })
  );
  assertValidEnvelope(graphRes, 'graph_query');

  // 8. impact_analyze
  const impactRes = await client.callTool(
    'impact_analyze',
    buildToolArgs(toolMap, 'impact_analyze', {
      workspace_id: workspaceId,
      path: fixtureDir,
      symbol_id: symbolId,
      id: symbolId,
      symbol: 'hash_secret',
      name: 'hash_secret',
      file_path: 'src/auth.rs',
      max_depth: 3
    })
  );
  assertValidEnvelope(impactRes, 'impact_analyze');

  // 9. context_build
  const contextRes = await client.callTool(
    'context_build',
    buildToolArgs(toolMap, 'context_build', {
      workspace_id: workspaceId,
      path: fixtureDir,
      query: 'how does verify_token validate and hash secrets?',
      token_budget: 2000,
      max_tokens: 2000,
      include_tests: true
    })
  );
  assertValidEnvelope(contextRes, 'context_build');

  // 10. job_status
  const jobArgs = {
    workspace_id: workspaceId,
    path: fixtureDir
  };
  if (jobId) {
    jobArgs.job_id = jobId;
    jobArgs.id = jobId;
  }
  const jobRes = await client.callTool(
    'job_status',
    buildToolArgs(toolMap, 'job_status', jobArgs)
  );
  assertValidEnvelope(jobRes, 'job_status');
}

describe('forge-codebase-mcp E2E suite', () => {
  let fixtureDir;
  let resolvedBin;

  before(() => {
    fixtureDir = createFixtureWorkspace();
  });

  after(() => {
    if (fixtureDir && fs.existsSync(fixtureDir)) {
      try {
        fs.rmSync(fixtureDir, { recursive: true, force: true });
      } catch (_) {
        // Ignore cleanup errors on locked temp files
      }
    }
  });

  test('resolveBinaryPath honors FORGE_MCP_BIN and resolves workspace binary', () => {
    const expectedName = getBinaryName();
    assert.ok(
      expectedName === 'forge-mcp.exe' || expectedName === 'forge-mcp',
      'getBinaryName() should return platform binary name'
    );

    const customOverride = path.join(fixtureDir, expectedName);
    fs.writeFileSync(customOverride, 'stub', 'utf8');
    const fromEnv = resolveBinaryPath({
      env: { ...process.env, FORGE_MCP_BIN: customOverride },
      mustExist: true
    });
    assert.equal(fromEnv, path.resolve(customOverride));
    fs.unlinkSync(customOverride);

    // Honor FORGE_MCP_BIN when set so the suite also runs outside the Rust workspace
    resolvedBin = resolveBinaryPath({ env: process.env });
    assert.ok(resolvedBin && fs.existsSync(resolvedBin), `Resolved binary must exist: ${resolvedBin}`);
  });

  test('ForgeMcpClient over stdio exercises all MCP tools end-to-end', async () => {
    const dbPath = path.join(fixtureDir, 'forge-stdio.db');
    const helpOut = spawnSync(resolvedBin || resolveBinaryPath(), ['--help'], {
      encoding: 'utf8'
    });
    const helpText = `${helpOut.stdout || ''}\n${helpOut.stderr || ''}`;

    const args = [];
    if (helpText.includes('--transport')) {
      args.push('--transport', 'stdio');
    }
    if (helpText.includes('--allow-root')) {
      args.push('--allow-root', fixtureDir);
    }
    if (helpText.includes('--db-path')) {
      args.push('--db-path', dbPath);
    } else if (helpText.includes('--database')) {
      args.push('--database', dbPath);
    }

    // Exercise via the CLI wrapper script `npm/bin/forge-mcp.js` using process.execPath
    const cliScript = path.resolve(__dirname, '..', 'bin', 'forge-mcp.js');
    const client = new ForgeMcpClient({
      transport: 'stdio',
      binPath: process.execPath,
      args: [cliScript, ...args],
      cwd: fixtureDir,
      env: {
        ...process.env,
        FORGE_DB_PATH: dbPath
      }
    });

    try {
      await exerciseFullToolSuite(client, fixtureDir);
    } finally {
      await client.close();
    }
  });

  test('ForgeMcpClient over HTTP enforces Bearer auth and exercises all MCP tools', async () => {
    const port = await getAvailablePort();
    const secretToken = 'forge-e2e-secret-token-99';
    const dbPath = path.join(fixtureDir, 'forge-http.db');
    const binPath = resolvedBin || resolveBinaryPath();
    const helpOut = spawnSync(binPath, ['--help'], { encoding: 'utf8' });
    const helpText = `${helpOut.stdout || ''}\n${helpOut.stderr || ''}`;

    const serverArgs = [];
    if (helpText.includes('--transport')) {
      serverArgs.push('--transport', 'http');
    }
    if (helpText.includes('--host')) {
      serverArgs.push('--host', '127.0.0.1');
    }
    if (helpText.includes('--port')) {
      serverArgs.push('--port', String(port));
    }
    if (helpText.includes('--auth-token')) {
      serverArgs.push('--auth-token', secretToken);
    }
    if (helpText.includes('--allow-root')) {
      serverArgs.push('--allow-root', fixtureDir);
    }
    if (helpText.includes('--db-path')) {
      serverArgs.push('--db-path', dbPath);
    } else if (helpText.includes('--database')) {
      serverArgs.push('--database', dbPath);
    }

    const serverProc = spawnForgeServer(serverArgs, {
      binPath,
      cwd: fixtureDir,
      env: {
        ...process.env,
        FORGE_AUTH_TOKEN: secretToken,
        FORGE_DB_PATH: dbPath
      },
      stdio: ['ignore', 'pipe', 'pipe'],
      forwardSignals: false
    });

    try {
      // Wait for HTTP server readiness
      const baseUrl = `http://127.0.0.1:${port}`;
      let ready = false;
      for (let i = 0; i < 80; i++) {
        try {
          const res = await fetch(`${baseUrl}/health`, {
            signal: AbortSignal.timeout(500)
          });
          if (res.status === 200 || res.status === 401 || res.status === 404) {
            ready = true;
            break;
          }
        } catch (_) {
          // Server still starting up
        }
        await new Promise((r) => setTimeout(r, 100));
      }
      assert.ok(ready, `Forge HTTP server failed to start on ${baseUrl}`);

      // Verify unauthenticated request is rejected with 401/403
      const unauthClient = new ForgeMcpClient({
        transport: 'http',
        url: `${baseUrl}/mcp`
      });
      await assert.rejects(
        async () => {
          await unauthClient.listTools();
        },
        (err) => {
          assert.ok(
            err.status === 401 || err.status === 403 || /401|403|unauthorized|forbidden/i.test(err.message),
            `Expected 401/403 error for missing Bearer token, got: ${err.message}`
          );
          return true;
        }
      );

      // Verify wrong token is rejected with 401/403
      const wrongTokenClient = new ForgeMcpClient({
        transport: 'http',
        url: `${baseUrl}/mcp`,
        token: 'invalid-token'
      });
      await assert.rejects(
        async () => {
          await wrongTokenClient.listTools();
        },
        (err) => {
          assert.ok(
            err.status === 401 || err.status === 403 || /401|403|unauthorized|forbidden/i.test(err.message),
            `Expected 401/403 error for wrong Bearer token, got: ${err.message}`
          );
          return true;
        }
      );

      // Verify authenticated client exercises full tool suite
      const authClient = new ForgeMcpClient({
        transport: 'http',
        url: `${baseUrl}/mcp`,
        token: secretToken
      });
      try {
        await exerciseFullToolSuite(authClient, fixtureDir);
      } finally {
        await authClient.close();
      }
    } finally {
      if (serverProc && serverProc.exitCode === null && !serverProc.killed) {
        try {
          serverProc.kill('SIGTERM');
        } catch (_) {
          // Ignore
        }
      }
    }
  });
});

describe('forge-codebase-mcp offline regressions', () => {
  const { findBinaryInPath, normalizeToolResult, parseHttpRpcBody } = require('../index.js');

  test('resolveBinaryPath order: explicit > bundled > release > debug > cache > PATH (never builds or downloads)', () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'forge-mcp-resolve-'));
    const binName = getBinaryName();
    const npmRoot = path.join(root, 'npmroot');
    const wsRoot = path.join(root, 'wsroot');
    const pathDir = path.join(root, 'pathdir');
    const emptyDir = path.join(root, 'empty');
    // A private cache and no downloads: the user's ~/.forge-mcp cache and the
    // published GitHub release must never leak into this test.
    const baseEnv = {
      PATH: pathDir,
      FORGE_MCP_CACHE_DIR: path.join(root, 'cache'),
      FORGE_MCP_SKIP_DOWNLOAD: '1'
    };
    const bundled = path.join(npmRoot, 'bin', 'native', binName);
    const release = path.join(wsRoot, 'target', 'release', binName);
    const debug = path.join(wsRoot, 'target', 'debug', binName);
    const cached = getCachedBinaryPath({ version: PACKAGE_VERSION, env: baseEnv });
    const onPath = path.join(pathDir, binName);
    for (const file of [bundled, release, debug, cached, onPath]) {
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, 'stub', 'utf8');
    }
    fs.mkdirSync(emptyDir, { recursive: true });
    // No Cargo.toml + autoBuild:false guarantees the cargo build step is never reached
    const base = { packageRoot: npmRoot, workspaceRoot: wsRoot, autoBuild: false, env: baseEnv };

    try {
      assert.ok(!fs.existsSync(path.join(wsRoot, 'Cargo.toml')));
      const explicit = path.join(root, 'explicit', binName);
      const fromEnv = path.join(root, 'env', binName);
      assert.equal(resolveBinaryPath({ ...base, binPath: explicit }), path.resolve(explicit));
      assert.equal(
        resolveBinaryPath({ ...base, env: { ...baseEnv, FORGE_MCP_BIN: fromEnv } }),
        path.resolve(fromEnv)
      );
      assert.equal(
        resolveBinaryPath({ ...base, binPath: explicit, env: { ...baseEnv, FORGE_MCP_BIN: fromEnv } }),
        path.resolve(explicit),
        'binPath option should take precedence over FORGE_MCP_BIN'
      );
      assert.equal(resolveBinaryPath(base), path.resolve(bundled));
      fs.unlinkSync(bundled);
      assert.equal(resolveBinaryPath(base), path.resolve(release));
      fs.unlinkSync(release);
      assert.equal(resolveBinaryPath(base), path.resolve(debug));
      fs.unlinkSync(debug);
      assert.equal(resolveBinaryPath(base), path.resolve(cached));
      fs.unlinkSync(cached);
      assert.equal(resolveBinaryPath(base), path.resolve(onPath));

      assert.throws(
        () => resolveBinaryPath({ ...base, env: { ...baseEnv, PATH: emptyDir } }),
        (err) => {
          for (const fragment of ['FORGE_MCP_BIN', bundled, release, debug, cached, 'system PATH', 'download skipped']) {
            assert.ok(err.message.includes(fragment), `error should mention ${fragment}: ${err.message}`);
          }
          return true;
        }
      );
      assert.throws(
        () =>
          resolveBinaryPath({
            ...base,
            env: { ...baseEnv, FORGE_MCP_BIN: path.join(root, 'missing', binName) },
            mustExist: true
          }),
        /non-existent file/
      );
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  test('findBinaryInPath / getBinaryName honor the platform argument', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'forge-mcp-path-'));
    try {
      fs.writeFileSync(path.join(dir, 'forge-mcp.exe'), 'stub', 'utf8');
      assert.equal(
        findBinaryInPath('forge-mcp', { PATH: dir }, 'win32'),
        path.resolve(dir, 'forge-mcp.exe')
      );
      assert.equal(findBinaryInPath('forge-mcp', { PATH: '' }, 'win32'), null);
      assert.equal(getBinaryName('linux'), 'forge-mcp');
      assert.equal(getBinaryName('darwin'), 'forge-mcp');
      assert.equal(getBinaryName('win32'), 'forge-mcp.exe');
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test('normalizeToolResult exposes envelope fields for every response shape', () => {
    const envelope = {
      snapshot_id: 'snap-1',
      freshness: 'current',
      results: [{ workspace_id: 'ws-1' }],
      evidence: [],
      limitations: [],
      truncated: false
    };

    const fromText = normalizeToolResult({ content: [{ type: 'text', text: JSON.stringify(envelope) }] });
    assert.equal(fromText.snapshot_id, 'snap-1');
    assert.equal(fromText.freshness, 'current');
    assert.deepEqual(fromText.results, envelope.results);
    assert.deepEqual(fromText.envelope, envelope);
    assert.ok(Array.isArray(fromText.content));

    // Regression: structuredContent without a content array used to be returned untouched
    const fromStructured = normalizeToolResult({ structuredContent: envelope });
    assert.equal(fromStructured.snapshot_id, 'snap-1');
    assert.equal(fromStructured.freshness, 'current');
    assert.deepEqual(fromStructured.results, envelope.results);
    assert.deepEqual(fromStructured.envelope, envelope);

    const fromStructuredWithPlainText = normalizeToolResult({
      content: [{ type: 'text', text: 'not json' }],
      structuredContent: envelope
    });
    assert.deepEqual(fromStructuredWithPlainText.results, envelope.results);

    const fromRaw = normalizeToolResult(envelope);
    assert.equal(fromRaw.isError, false);
    assert.deepEqual(JSON.parse(fromRaw.content[0].text), envelope);
    assert.deepEqual(fromRaw.envelope, envelope);
    assert.equal(fromRaw.snapshot_id, 'snap-1');

    const toolError = { isError: true, content: [{ type: 'text', text: 'boom' }] };
    assert.deepEqual(normalizeToolResult(toolError), toolError);

    const errorWithJson = normalizeToolResult({
      isError: true,
      content: [{ type: 'text', text: JSON.stringify({ ...envelope, isError: false }) }]
    });
    assert.equal(errorWithJson.isError, true, 'MCP isError must win over envelope fields');

    assert.equal(normalizeToolResult(null), null);
    assert.equal(normalizeToolResult('text'), 'text');
  });

  test('parseHttpRpcBody handles JSON and text/event-stream bodies', () => {
    assert.deepEqual(parseHttpRpcBody('', 'application/json'), {});
    assert.deepEqual(
      parseHttpRpcBody('{"jsonrpc":"2.0","id":1,"result":{"ok":true}}', 'application/json', 1),
      { jsonrpc: '2.0', id: 1, result: { ok: true } }
    );

    const sse = [
      'event: message',
      'data: {"jsonrpc":"2.0","id":99,"result":{"decoy":true}}',
      '',
      'event: message',
      'data: {"jsonrpc":"2.0","id":2,"result":{"ok":true}}',
      '',
      'data: [DONE]',
      ''
    ].join('\n');
    assert.deepEqual(parseHttpRpcBody(sse, 'text/event-stream', 2).result, { ok: true });
    assert.deepEqual(parseHttpRpcBody(sse, 'text/event-stream').result, { ok: true });

    // CRLF framing, no event: line, no content-type: detected from the data: prefix
    const crlf = 'data: {"jsonrpc":"2.0","id":3,"error":{"code":-32000,"message":"boom"}}\r\n\r\n';
    assert.deepEqual(parseHttpRpcBody(crlf, '', 3).error, { code: -32000, message: 'boom' });
  });

  describe('ForgeMcpClient.close() terminates the stdio child', () => {
    let scriptDir;
    const writeFakeServer = (name, exitOnEof) => {
      const file = path.join(scriptDir, name);
      fs.writeFileSync(
        file,
        [
          "const rl = require('node:readline').createInterface({ input: process.stdin });",
          "rl.on('line', (line) => {",
          '  const msg = JSON.parse(line);',
          '  if (msg.id !== undefined) {',
          "    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: msg.id, result: { serverInfo: { name: 'fake', version: '0' } } }) + '\\n');",
          '  }',
          '});',
          exitOnEof ? "rl.on('close', () => process.exit(0));" : 'setInterval(() => {}, 1000);',
          ''
        ].join('\n'),
        'utf8'
      );
      return file;
    };

    before(() => {
      scriptDir = fs.mkdtempSync(path.join(os.tmpdir(), 'forge-mcp-close-'));
    });

    after(() => {
      fs.rmSync(scriptDir, { recursive: true, force: true });
    });

    test('graceful: child exits with code 0 on stdin EOF', async () => {
      const script = writeFakeServer('graceful.js', true);
      const client = new ForgeMcpClient({ transport: 'stdio', binPath: process.execPath, args: [script] });
      await client.initialize();
      const child = client.child;
      const started = Date.now();
      await client.close();
      assert.ok(Date.now() - started < 3000, 'close() should finish within 3 s');
      assert.equal(child.exitCode, 0);
      await assert.rejects(() => client.request('tools/list'), /closed/);
    });

    test('stubborn: child ignoring stdin EOF is terminated by signal', async () => {
      const script = writeFakeServer('stubborn.js', false);
      const client = new ForgeMcpClient({
        transport: 'stdio',
        binPath: process.execPath,
        args: [script],
        closeGraceMs: 200
      });
      await client.initialize();
      const child = client.child;
      const started = Date.now();
      await client.close();
      assert.ok(Date.now() - started < 3000, 'close() should finish within 3 s');
      assert.ok(child.exitCode !== null || child.signalCode !== null, 'child must have exited');
    });

    test('real forge-mcp binary exits with code 0 on close()', async (t) => {
      let binPath;
      try {
        binPath = resolveBinaryPath({ autoBuild: false, autoDownload: false, mustExist: true });
      } catch (_) {
        t.skip('forge-mcp binary not available (set FORGE_MCP_BIN)');
        return;
      }
      const root = fs.mkdtempSync(path.join(os.tmpdir(), 'forge-mcp-close-real-'));
      const client = new ForgeMcpClient({
        transport: 'stdio',
        binPath,
        allowRoot: root,
        dbPath: path.join(root, 'forge.db')
      });
      try {
        await client.initialize();
        const child = client.child;
        const started = Date.now();
        await client.close();
        assert.ok(Date.now() - started < 3000, 'close() should finish within 3 s');
        assert.equal(child.exitCode, 0);
      } finally {
        await client.close();
        try {
          fs.rmSync(root, { recursive: true, force: true });
        } catch (_) {
          // Ignore locked temp files on Windows
        }
      }
    });
  });
});
