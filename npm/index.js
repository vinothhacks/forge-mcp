'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { spawn, spawnSync } = require('node:child_process');
const readline = require('node:readline');
const { EventEmitter } = require('node:events');
const { getTargetTriple, getCachedBinaryPath } = require('./lib/platform.js');
const { downloadBinary } = require('./lib/download.js');

const PACKAGE_VERSION = require('./package.json').version;

function isTruthyEnv(value) {
  if (value === undefined || value === null) return false;
  const v = String(value).trim().toLowerCase();
  return v !== '' && v !== '0' && v !== 'false' && v !== 'no';
}

/**
 * Returns the platform-specific executable name for Forge MCP.
 * @param {string} [platform=process.platform]
 * @returns {string}
 */
function getBinaryName(platform = process.platform) {
  return platform === 'win32' ? 'forge-mcp.exe' : 'forge-mcp';
}

/**
 * Searches the system PATH for the given executable name.
 * @param {string} binName
 * @param {NodeJS.ProcessEnv} [env=process.env]
 * @param {string} [platform=process.platform]
 * @returns {string|null}
 */
function findBinaryInPath(binName, env = process.env, platform = process.platform) {
  const pathVar = env.PATH || env.Path || env.path || '';
  if (!pathVar) {
    return null;
  }

  const dirs = pathVar.split(path.delimiter).filter(Boolean);
  const candidates = platform === 'win32' && !binName.toLowerCase().endsWith('.exe')
    ? [binName, `${binName}.exe`]
    : [binName];

  for (const dir of dirs) {
    for (const name of candidates) {
      const candidate = path.join(dir, name);
      try {
        if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) {
          return path.resolve(candidate);
        }
      } catch (_) {
        // Ignore inaccessible PATH entries
      }
    }
  }

  return null;
}

/**
 * Resolves the compiled `forge-mcp` (or `forge-mcp.exe` on Windows) binary path.
 *
 * Resolution order:
 * 1) `FORGE_MCP_BIN` environment variable (or `options.binPath`)
 * 2) Bundled `npm/bin/native/forge-mcp[.exe]`
 * 3) Workspace `target/release/forge-mcp[.exe]` or `target/debug/forge-mcp[.exe]`
 * 4) Previously downloaded release binary in the cache (`~/.forge-mcp/bin/<version>/<target>/`)
 * 5) System `PATH`
 * 6) Download the matching release binary from GitHub Releases, verify it against
 *    `SHA256SUMS`, and cache it (skipped when `FORGE_MCP_SKIP_DOWNLOAD` is set or
 *    `options.autoDownload === false`)
 * 7) If not found and `Cargo.toml` exists in the parent workspace directory,
 *    automatically builds with `cargo build --release` (emitting all build logs
 *    to `stderr` so `stdio` JSON-RPC `stdout` is never polluted).
 *
 * @param {object} [options={}]
 * @param {string} [options.binPath] Explicit binary path override
 * @param {NodeJS.ProcessEnv} [options.env=process.env] Environment variables
 * @param {string} [options.platform=process.platform] Target OS platform
 * @param {string} [options.packageRoot] Path to `npm/` directory
 * @param {string} [options.workspaceRoot] Path to Rust workspace root directory
 * @param {boolean} [options.autoBuild=true] Whether to run `cargo build --release` if missing
 * @param {boolean} [options.autoDownload=true] Whether to download a release binary if missing
 * @param {string} [options.version] Release version to download (defaults to this package's version)
 * @param {boolean} [options.mustExist=false] Whether an explicit `FORGE_MCP_BIN` path must exist on disk
 * @param {NodeJS.WritableStream} [options.stderr=process.stderr] Stream for build logs
 * @returns {string} Absolute path to the resolved binary
 */
function resolveBinaryPath(options = {}) {
  const env = options.env || process.env;
  const platform = options.platform || process.platform;
  const packageRoot = options.packageRoot
    ? path.resolve(options.packageRoot)
    : __dirname;
  const workspaceRoot = options.workspaceRoot
    ? path.resolve(options.workspaceRoot)
    : path.resolve(packageRoot, '..');
  const autoBuild = options.autoBuild !== false;
  const stderr = options.stderr || process.stderr;
  const binName = getBinaryName(platform);

  // 1) Explicit option or FORGE_MCP_BIN env var
  const explicitBin = options.binPath || env.FORGE_MCP_BIN;
  if (explicitBin && String(explicitBin).trim().length > 0) {
    const resolvedExplicit = path.resolve(String(explicitBin).trim());
    if (!options.mustExist || fs.existsSync(resolvedExplicit)) {
      return resolvedExplicit;
    }
    throw new Error(`FORGE_MCP_BIN points to a non-existent file: ${resolvedExplicit}`);
  }

  // 2) Bundled npm/bin/native/forge-mcp[.exe]
  const bundledCandidate = path.join(packageRoot, 'bin', 'native', binName);
  if (fs.existsSync(bundledCandidate) && fs.statSync(bundledCandidate).isFile()) {
    return path.resolve(bundledCandidate);
  }

  // 3) Workspace target/release/forge-mcp[.exe] or target/debug/forge-mcp[.exe]
  const releaseCandidate = path.join(workspaceRoot, 'target', 'release', binName);
  if (fs.existsSync(releaseCandidate) && fs.statSync(releaseCandidate).isFile()) {
    return path.resolve(releaseCandidate);
  }

  const debugCandidate = path.join(workspaceRoot, 'target', 'debug', binName);
  if (fs.existsSync(debugCandidate) && fs.statSync(debugCandidate).isFile()) {
    return path.resolve(debugCandidate);
  }

  // 4) Previously downloaded release binary
  const version = options.version || PACKAGE_VERSION;
  let cachedCandidate = null;
  try {
    cachedCandidate = getCachedBinaryPath({ version, platform, env });
  } catch (_) {
    // Unsupported platform: no prebuilt binary exists, continue with PATH / cargo
  }
  if (cachedCandidate && fs.existsSync(cachedCandidate) && fs.statSync(cachedCandidate).isFile()) {
    return cachedCandidate;
  }

  // 5) System PATH
  const pathCandidate = findBinaryInPath(binName, env, platform);
  if (pathCandidate) {
    return pathCandidate;
  }

  // 6) Download the release binary (synchronously, via a child node process)
  const skipDownload = isTruthyEnv(env.FORGE_MCP_SKIP_DOWNLOAD) || options.autoDownload === false;
  if (!skipDownload && cachedCandidate) {
    const dl = spawnSync(
      process.execPath,
      [path.join(__dirname, 'lib', 'download.js'), '--version', version, '--dest', cachedCandidate],
      {
        env: { ...process.env, ...env },
        stdio: ['ignore', 'pipe', 'pipe'],
        windowsHide: true
      }
    );
    if (stderr && typeof stderr.write === 'function') {
      if (dl.stdout && dl.stdout.length > 0) stderr.write(dl.stdout);
      if (dl.stderr && dl.stderr.length > 0) stderr.write(dl.stderr);
    }
    if (dl.status === 0 && fs.existsSync(cachedCandidate)) {
      return cachedCandidate;
    }
  }

  // 7) Automatic cargo build --release if Cargo.toml exists in parent directory
  const cargoTomlPath = path.join(workspaceRoot, 'Cargo.toml');
  if (autoBuild && fs.existsSync(cargoTomlPath)) {
    if (stderr && typeof stderr.write === 'function') {
      stderr.write(
        `[forge-mcp] Native binary not found; building from ${cargoTomlPath} with "cargo build --release"...\n`
      );
    }

    // Redirect both stdout and stderr from cargo to stderr so stdio JSON-RPC stdout is never polluted
    const buildResult = spawnSync('cargo', ['build', '--release'], {
      cwd: workspaceRoot,
      env: { ...process.env, ...env },
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true
    });

    if (buildResult.stdout && buildResult.stdout.length > 0 && stderr && typeof stderr.write === 'function') {
      stderr.write(buildResult.stdout);
    }
    if (buildResult.stderr && buildResult.stderr.length > 0 && stderr && typeof stderr.write === 'function') {
      stderr.write(buildResult.stderr);
    }

    if (buildResult.error) {
      throw new Error(`Failed to execute cargo build --release: ${buildResult.error.message}`);
    }
    if (buildResult.status !== 0) {
      throw new Error(`cargo build --release exited with status ${buildResult.status}`);
    }

    if (fs.existsSync(releaseCandidate) && fs.statSync(releaseCandidate).isFile()) {
      return path.resolve(releaseCandidate);
    }
  }

  throw new Error(
    `Unable to locate "${binName}". Checked FORGE_MCP_BIN, ${bundledCandidate}, ${releaseCandidate}, ${debugCandidate}, ` +
      `${cachedCandidate || 'the download cache (unsupported platform)'}, and system PATH` +
      (skipDownload ? ' (download skipped: FORGE_MCP_SKIP_DOWNLOAD or autoDownload:false).' : '; the release download did not succeed.') +
      ' Install with "cargo install forge-mcp --locked" or set FORGE_MCP_BIN.'
  );
}

/**
 * Spawns the Forge MCP native server binary with the given CLI arguments and options.
 *
 * @param {string[]} [args=[]] CLI arguments forwarded to `forge-mcp`
 * @param {object} [options={}]
 * @param {string} [options.binPath] Explicit binary path override
 * @param {string} [options.cwd=process.cwd()] Working directory
 * @param {NodeJS.ProcessEnv} [options.env] Environment variables merged with `process.env`
 * @param {import('node:child_process').StdioOptions} [options.stdio='inherit'] Child stdio configuration
 * @param {boolean} [options.forwardSignals=true] Forward SIGINT and SIGTERM to child process
 * @returns {import('node:child_process').ChildProcess & { binPath: string }}
 */
function spawnForgeServer(args = [], options = {}) {
  const binPath = resolveBinaryPath(options);
  const childEnv = { ...process.env, ...(options.env || {}) };
  const stdio = options.stdio || ['pipe', 'pipe', 'pipe'];
  const forwardSignals = options.forwardSignals !== false;

  const child = spawn(binPath, args, {
    cwd: options.cwd || process.cwd(),
    env: childEnv,
    stdio,
    windowsHide: true
  });

  child.binPath = binPath;

  if (forwardSignals) {
    const handleSigInt = () => {
      if (!child.killed && child.exitCode === null) {
        try {
          child.kill('SIGINT');
        } catch (_) {
          // Ignore if already exited
        }
      }
    };

    const handleSigTerm = () => {
      if (!child.killed && child.exitCode === null) {
        try {
          child.kill('SIGTERM');
        } catch (_) {
          // Ignore if already exited
        }
      }
    };

    process.on('SIGINT', handleSigInt);
    process.on('SIGTERM', handleSigTerm);

    child.once('exit', () => {
      process.removeListener('SIGINT', handleSigInt);
      process.removeListener('SIGTERM', handleSigTerm);
    });
  }

  return child;
}

/**
 * Normalizes an MCP `tools/call` response so callers can access both the standard
 * MCP `{ content, isError }` fields and the parsed Forge response envelope fields
 * (`snapshot_id`, `freshness`, `results`, `evidence`, `limitations`, `truncated`, `next_cursor`).
 *
 * @param {any} result
 * @returns {any}
 */
function normalizeToolResult(result) {
  if (!result || typeof result !== 'object') {
    return result;
  }

  if (Array.isArray(result.content)) {
    const textBlock = result.content.find(
      (item) => item && item.type === 'text' && typeof item.text === 'string'
    );
    if (textBlock) {
      try {
        const parsed = JSON.parse(textBlock.text);
        if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
          return Object.assign({}, parsed, result, { envelope: parsed });
        }
      } catch (_) {
        // Non-JSON text block; fall back to structuredContent below
      }
    }
  }

  // Servers may carry the envelope only in `structuredContent` (with or without `content`)
  if (
    result.structuredContent &&
    typeof result.structuredContent === 'object' &&
    !Array.isArray(result.structuredContent)
  ) {
    return Object.assign({}, result.structuredContent, result, {
      envelope: result.structuredContent
    });
  }

  if (Array.isArray(result.content)) {
    return result;
  }

  // If the server returned the envelope directly, also provide MCP-compatible `content`
  if ('snapshot_id' in result || 'results' in result || 'evidence' in result) {
    return Object.assign(
      {
        content: [{ type: 'text', text: JSON.stringify(result) }],
        isError: false,
        envelope: result
      },
      result
    );
  }

  return result;
}

/**
 * Parses an HTTP response body that may be either `application/json` or `text/event-stream`.
 *
 * @param {string} bodyText
 * @param {string} contentType
 * @param {number|string} [expectedId]
 * @returns {any}
 */
function parseHttpRpcBody(bodyText, contentType = '', expectedId = undefined) {
  const trimmed = (bodyText || '').trim();
  if (!trimmed) {
    return {};
  }

  const isSse =
    contentType.toLowerCase().includes('text/event-stream') ||
    trimmed.startsWith('event:') ||
    trimmed.startsWith('data:');

  if (!isSse) {
    return JSON.parse(trimmed);
  }

  const messages = [];
  for (const rawLine of trimmed.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (line.startsWith('data:')) {
      const dataPayload = line.slice(5).trim();
      if (dataPayload && dataPayload !== '[DONE]') {
        try {
          messages.push(JSON.parse(dataPayload));
        } catch (_) {
          // Ignore non-JSON SSE data lines
        }
      }
    }
  }

  if (expectedId !== undefined) {
    const matching = messages.find((m) => m && m.id === expectedId);
    if (matching) {
      return matching;
    }
  }

  if (messages.length > 0) {
    return messages[messages.length - 1];
  }

  return JSON.parse(trimmed);
}

/**
 * Programmatic MCP client for Forge supporting both `stdio` and `http` transports.
 */
class ForgeMcpClient extends EventEmitter {
  /**
   * @param {object} [options={}]
   * @param {'stdio'|'http'} [options.transport] Transport mode ('stdio' or 'http')
   * @param {string} [options.binPath] Path to `forge-mcp` binary (for stdio or spawned http)
   * @param {string[]} [options.args] Extra CLI arguments for `stdio` server
   * @param {string} [options.cwd] Working directory for spawned server
   * @param {NodeJS.ProcessEnv} [options.env] Environment variables for spawned server
   * @param {string} [options.allowRoot] Optional `--allow-root` directory passed to spawned server
   * @param {string} [options.dbPath] Optional `--db-path` passed to spawned server
   * @param {string} [options.url] HTTP endpoint URL (e.g. `http://127.0.0.1:8080/mcp` or `http://127.0.0.1:8080`)
   * @param {string} [options.host='127.0.0.1'] HTTP host when `url` is not specified
   * @param {number} [options.port=8080] HTTP port when `url` is not specified
   * @param {string} [options.token] Optional Bearer token for HTTP authentication
   * @param {string} [options.authToken] Alias for `options.token`
   * @param {string} [options.bearerToken] Alias for `options.token`
   * @param {Record<string, string>} [options.headers] Additional HTTP headers
   * @param {number} [options.timeoutMs=60000] Per-request timeout in milliseconds
   * @param {boolean} [options.inheritStderr=false] Forward child stderr to process.stderr
   * @param {number} [options.closeGraceMs=1000] How long close() waits for the stdio server to exit
   *   on stdin EOF before sending SIGTERM (SIGKILL follows 2000 ms later)
   */
  constructor(options = {}) {
    super();
    this.options = { ...options };
    this.transport =
      options.transport || (options.url || options.port || options.host ? 'http' : 'stdio');
    this.timeoutMs = options.timeoutMs || 60000;
    this.nextId = 1;
    this.pending = new Map();
    this.closed = false;
    this.initialized = false;
    this.serverInfo = null;
    this.sessionId = null;
    this.stderrChunks = [];

    if (this.transport === 'http') {
      if (options.url) {
        const parsed = new URL(options.url);
        if (!parsed.pathname || parsed.pathname === '/') {
          this.baseUrl = options.url.replace(/\/+$/, '');
          this.mcpUrl = `${this.baseUrl}/mcp`;
        } else {
          this.mcpUrl = options.url;
          this.baseUrl = `${parsed.protocol}//${parsed.host}`;
        }
      } else {
        const host = options.host || '127.0.0.1';
        const port = options.port || 8080;
        this.baseUrl = `http://${host}:${port}`;
        this.mcpUrl = `${this.baseUrl}/mcp`;
      }
      this.token =
        options.token !== undefined
          ? options.token
          : options.authToken !== undefined
            ? options.authToken
            : options.bearerToken !== undefined
              ? options.bearerToken
              : null;
      this.headers = { ...(options.headers || {}) };
    } else if (this.transport === 'stdio') {
      const baseArgs = Array.isArray(options.args)
        ? [...options.args]
        : ['--transport', 'stdio'];
      if (options.allowRoot && !baseArgs.includes('--allow-root')) {
        baseArgs.push('--allow-root', path.resolve(options.allowRoot));
      }
      if (options.dbPath && !baseArgs.includes('--db-path')) {
        baseArgs.push('--db-path', path.resolve(options.dbPath));
      }
      this.args = baseArgs;
      this.child = null;
      this.rl = null;
    } else {
      throw new Error(`Unsupported transport: ${this.transport}. Expected "stdio" or "http".`);
    }
  }

  /**
   * Ensures the underlying `stdio` process is spawned and line reader is attached.
   */
  _ensureStdioProcess() {
    if (this.closed) {
      throw new Error('ForgeMcpClient is closed');
    }
    if (this.child) {
      return;
    }

    this.child = spawnForgeServer(this.args, {
      binPath: this.options.binPath,
      cwd: this.options.cwd,
      env: this.options.env,
      packageRoot: this.options.packageRoot,
      workspaceRoot: this.options.workspaceRoot,
      stdio: ['pipe', 'pipe', 'pipe'],
      forwardSignals: false
    });

    if (this.child.stderr) {
      this.child.stderr.on('data', (chunk) => {
        const text = chunk.toString('utf8');
        this.stderrChunks.push(text);
        if (this.stderrChunks.length > 100) {
          this.stderrChunks.shift();
        }
        if (this.options.inheritStderr) {
          process.stderr.write(chunk);
        }
        this.emit('stderr', text);
      });
    }

    this.rl = readline.createInterface({
      input: this.child.stdout,
      crlfDelay: Infinity
    });

    this.rl.on('line', (line) => {
      const trimmed = line.trim();
      if (!trimmed) {
        return;
      }

      let message;
      try {
        message = JSON.parse(trimmed);
      } catch (_) {
        this.emit('stdout:nonjson', trimmed);
        return;
      }

      if (message && message.id !== undefined && message.id !== null && this.pending.has(message.id)) {
        const entry = this.pending.get(message.id);
        this.pending.delete(message.id);
        clearTimeout(entry.timer);

        if (message.error) {
          const err = new Error(message.error.message || 'JSON-RPC error');
          err.code = message.error.code;
          err.data = message.error.data;
          err.rpcError = message.error;
          entry.reject(err);
        } else {
          entry.resolve(message.result);
        }
      } else if (message && message.method) {
        this.emit('notification', message);
      }
    });

    this.child.on('error', (err) => {
      this._rejectAllPending(new Error(`Forge stdio process error: ${err.message}`));
    });

    this.child.on('exit', (code, signal) => {
      const stderrSummary = this.stderrChunks.join('').trim();
      const suffix = stderrSummary ? ` | stderr: ${stderrSummary.slice(-500)}` : '';
      this._rejectAllPending(
        new Error(`Forge stdio server exited (code=${code}, signal=${signal})${suffix}`)
      );
      this.emit('exit', code, signal);
    });
  }

  /**
   * Rejects all in-flight JSON-RPC requests.
   * @param {Error} error
   */
  _rejectAllPending(error) {
    for (const [, entry] of this.pending.entries()) {
      clearTimeout(entry.timer);
      entry.reject(error);
    }
    this.pending.clear();
  }

  /**
   * Sends a JSON-RPC 2.0 request over the configured transport.
   *
   * @param {string} method
   * @param {object} [params={}]
   * @returns {Promise<any>}
   */
  async request(method, params = {}) {
    if (this.closed) {
      throw new Error('ForgeMcpClient is closed');
    }

    const id = this.nextId++;
    const payload = {
      jsonrpc: '2.0',
      id,
      method,
      params: params ?? {}
    };

    if (this.transport === 'stdio') {
      this._ensureStdioProcess();
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          if (this.pending.has(id)) {
            this.pending.delete(id);
            reject(new Error(`JSON-RPC request "${method}" (id=${id}) timed out after ${this.timeoutMs}ms`));
          }
        }, this.timeoutMs);

        this.pending.set(id, { resolve, reject, timer, method });

        try {
          this.child.stdin.write(`${JSON.stringify(payload)}\n`, 'utf8');
        } catch (err) {
          clearTimeout(timer);
          this.pending.delete(id);
          reject(err);
        }
      });
    }

    // HTTP transport
    const headers = {
      'Content-Type': 'application/json',
      Accept: 'application/json, text/event-stream',
      ...this.headers
    };
    if (this.token) {
      headers.Authorization = `Bearer ${this.token}`;
    }
    if (this.sessionId) {
      headers['Mcp-Session-Id'] = this.sessionId;
    }

    const response = await fetch(this.mcpUrl, {
      method: 'POST',
      headers,
      body: JSON.stringify(payload),
      signal: AbortSignal.timeout(this.timeoutMs)
    });

    const returnedSessionId = response.headers.get('mcp-session-id');
    if (returnedSessionId) {
      this.sessionId = returnedSessionId;
    }

    const bodyText = await response.text();
    if (!response.ok) {
      const err = new Error(
        `HTTP ${response.status} ${response.statusText} from ${this.mcpUrl}: ${bodyText}`
      );
      err.status = response.status;
      err.statusText = response.statusText;
      err.body = bodyText;
      throw err;
    }

    const contentType = response.headers.get('content-type') || '';
    const rpcMessage = parseHttpRpcBody(bodyText, contentType, id);

    if (rpcMessage && rpcMessage.error) {
      const err = new Error(rpcMessage.error.message || 'JSON-RPC error');
      err.code = rpcMessage.error.code;
      err.data = rpcMessage.error.data;
      err.rpcError = rpcMessage.error;
      throw err;
    }

    return rpcMessage && 'result' in rpcMessage ? rpcMessage.result : rpcMessage;
  }

  /**
   * Sends a JSON-RPC 2.0 notification (no response expected).
   *
   * @param {string} method
   * @param {object} [params={}]
   * @returns {Promise<void>}
   */
  async notify(method, params = {}) {
    if (this.closed) {
      throw new Error('ForgeMcpClient is closed');
    }

    const payload = {
      jsonrpc: '2.0',
      method,
      params: params ?? {}
    };

    if (this.transport === 'stdio') {
      this._ensureStdioProcess();
      this.child.stdin.write(`${JSON.stringify(payload)}\n`, 'utf8');
      return;
    }

    const headers = {
      'Content-Type': 'application/json',
      Accept: 'application/json, text/event-stream',
      ...this.headers
    };
    if (this.token) {
      headers.Authorization = `Bearer ${this.token}`;
    }
    if (this.sessionId) {
      headers['Mcp-Session-Id'] = this.sessionId;
    }

    const response = await fetch(this.mcpUrl, {
      method: 'POST',
      headers,
      body: JSON.stringify(payload),
      signal: AbortSignal.timeout(this.timeoutMs)
    });

    if (!response.ok) {
      const bodyText = await response.text().catch(() => '');
      const err = new Error(
        `HTTP ${response.status} ${response.statusText} from ${this.mcpUrl}: ${bodyText}`
      );
      err.status = response.status;
      err.statusText = response.statusText;
      err.body = bodyText;
      throw err;
    }
  }

  /**
   * Performs the MCP `initialize` handshake and sends `notifications/initialized`.
   *
   * @param {object} [clientInfo={ name: 'forge-codebase-mcp', version: PACKAGE_VERSION }]
   * @returns {Promise<any>}
   */
  async initialize(clientInfo = { name: 'forge-codebase-mcp', version: PACKAGE_VERSION }) {
    const result = await this.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo
    });
    this.initialized = true;
    this.serverInfo = result;

    try {
      await this.notify('notifications/initialized', {});
    } catch (_) {
      // Some minimal stateless HTTP servers only handle request/response messages
    }

    return result;
  }

  /**
   * Lists all available MCP tools from the Forge server.
   *
   * @returns {Promise<{ tools: Array<object> }>}
   */
  async listTools() {
    const result = await this.request('tools/list', {});
    if (Array.isArray(result)) {
      return { tools: result };
    }
    return result;
  }

  /**
   * Invokes an MCP tool by name with the given arguments.
   * Returns an object containing both the standard MCP `{ content, isError }` fields
   * and the parsed Forge response envelope fields (`snapshot_id`, `freshness`,
   * `results`, `evidence`, `limitations`, `truncated`, `next_cursor`, `envelope`).
   *
   * @param {string} name Tool name (e.g. `workspace_open`, `code_search`)
   * @param {object} [args={}] Tool input arguments
   * @returns {Promise<any>}
   */
  async callTool(name, args = {}) {
    if (!name || typeof name !== 'string') {
      throw new Error('Tool name must be a non-empty string');
    }
    const rawResult = await this.request('tools/call', {
      name,
      arguments: args ?? {}
    });
    return normalizeToolResult(rawResult);
  }

  /**
   * Checks the `/health` endpoint when using HTTP transport.
   *
   * @returns {Promise<any>}
   */
  async health() {
    if (this.transport !== 'http') {
      throw new Error('health() is only supported for HTTP transport');
    }
    const response = await fetch(`${this.baseUrl}/health`, {
      method: 'GET',
      signal: AbortSignal.timeout(this.timeoutMs)
    });
    const text = await response.text();
    if (!response.ok) {
      const err = new Error(`HTTP ${response.status} from ${this.baseUrl}/health: ${text}`);
      err.status = response.status;
      throw err;
    }
    try {
      return JSON.parse(text);
    } catch (_) {
      return { status: 'ok', raw: text };
    }
  }

  /**
   * Closes the client connection and terminates any spawned stdio process.
   *
   * @returns {Promise<void>}
   */
  async close() {
    if (this.closed) {
      return;
    }
    this.closed = true;
    this._rejectAllPending(new Error('ForgeMcpClient closed'));

    if (this.rl) {
      this.rl.close();
      this.rl = null;
    }

    if (this.child) {
      const proc = this.child;
      this.child = null;

      if (proc.exitCode === null && proc.signalCode === null) {
        const graceMs =
          typeof this.options.closeGraceMs === 'number' && this.options.closeGraceMs >= 0
            ? this.options.closeGraceMs
            : 1000;
        await new Promise((resolve) => {
          let termTimer = null;
          let killTimer = null;
          const finish = () => {
            clearTimeout(termTimer);
            clearTimeout(killTimer);
            resolve();
          };

          proc.once('exit', finish);

          // 1) Graceful: close stdin so the server sees EOF, flushes state, and exits on its own
          try {
            if (proc.stdin && !proc.stdin.destroyed) {
              proc.stdin.end();
            }
          } catch (_) {
            // Ignore
          }

          // 2) SIGTERM if the server is still running after the grace period
          termTimer = setTimeout(() => {
            try {
              proc.kill('SIGTERM');
            } catch (_) {
              // Ignore
            }
          }, graceMs);

          // 3) SIGKILL as a last resort
          killTimer = setTimeout(() => {
            try {
              proc.kill('SIGKILL');
            } catch (_) {
              // Ignore
            }
            finish();
          }, graceMs + 2000);
        });
      }
    }
  }
}

module.exports = {
  getBinaryName,
  findBinaryInPath,
  resolveBinaryPath,
  spawnForgeServer,
  normalizeToolResult,
  parseHttpRpcBody,
  getTargetTriple,
  downloadBinary,
  ForgeMcpClient
};
