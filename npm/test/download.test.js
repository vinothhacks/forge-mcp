'use strict';

// Offline tests for the release-binary download path. A local HTTP server plays
// the role of GitHub Releases, so nothing leaves the machine.

const { test, describe, before, after } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const { spawn, spawnSync } = require('node:child_process');

// Static file server for <root>/<tag>/<asset>; prints PORT=<n> once listening.
const RELEASE_SERVER_SOURCE = `
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const root = process.argv[2];
const server = http.createServer((req, res) => {
  const rel = decodeURIComponent(req.url.split('?')[0]).replace(/^\\/+/, '');
  const file = path.join(root, rel);
  if (!file.startsWith(root) || !fs.existsSync(file) || !fs.statSync(file).isFile()) {
    res.writeHead(404).end();
    return;
  }
  res.writeHead(200).end(fs.readFileSync(file));
});
server.listen(0, '127.0.0.1', () => process.stdout.write('PORT=' + server.address().port + '\\n'));
`;

const {
  getTargetTriple,
  getArchiveExt,
  getAssetName,
  getCachedBinaryPath,
  getTarCommand
} = require('../lib/platform.js');
const { downloadBinary, parseSha256Sums } = require('../lib/download.js');

const VERSION = '9.9.9-test';
const TAG = `v${VERSION}`;
const triple = getTargetTriple();
const ext = getArchiveExt();
const asset = getAssetName(VERSION, triple, ext);
const binName = process.platform === 'win32' ? 'forge-mcp.exe' : 'forge-mcp';
const STUB = 'forge-mcp stub binary for download test\n';

let tmp;
let server;
let baseUrl;
let goodSums;

function buildArchive(dir) {
  const stage = path.join(dir, 'stage');
  const inner = path.join(stage, `forge-mcp-v${VERSION}-${triple}`);
  fs.mkdirSync(inner, { recursive: true });
  fs.writeFileSync(path.join(inner, binName), STUB);
  const out = path.join(dir, 'release', TAG);
  fs.mkdirSync(out, { recursive: true });
  const archive = path.join(out, asset);
  const args = ext === 'zip'
    ? ['-a', '-cf', archive, '-C', stage, path.basename(inner)]
    : ['-czf', archive, '-C', stage, path.basename(inner)];
  const res = spawnSync(getTarCommand(), args, { stdio: 'pipe' });
  assert.equal(res.status, 0, `tar failed: ${res.stderr}`);
  const digest = crypto.createHash('sha256').update(fs.readFileSync(archive)).digest('hex');
  goodSums = `${digest}  ${asset}\n`;
  fs.writeFileSync(path.join(out, 'SHA256SUMS'), goodSums);
  return out;
}

describe('release binary download', () => {
  before(async () => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'forge-dl-test-'));
    buildArchive(tmp);
    // The fake release server runs in its own process: resolveBinaryPath() downloads
    // through spawnSync, which blocks this process's event loop until it finishes.
    const serverScript = path.join(tmp, 'release-server.js');
    fs.writeFileSync(serverScript, RELEASE_SERVER_SOURCE);
    server = spawn(process.execPath, [serverScript, path.join(tmp, 'release')], {
      stdio: ['ignore', 'pipe', 'inherit']
    });
    const port = await new Promise((resolve, reject) => {
      let out = '';
      server.stdout.on('data', (d) => {
        out += d;
        const m = /PORT=(\d+)/.exec(out);
        if (m) resolve(Number(m[1]));
      });
      server.once('exit', (code) => reject(new Error(`release server exited early (${code})`)));
    });
    baseUrl = `http://127.0.0.1:${port}`;
  });

  after(() => {
    server.kill();
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  test('getTargetTriple maps supported platforms and rejects others', () => {
    assert.equal(getTargetTriple('win32', 'x64'), 'x86_64-pc-windows-msvc');
    assert.equal(getTargetTriple('linux', 'x64'), 'x86_64-unknown-linux-gnu');
    assert.equal(getTargetTriple('linux', 'arm64'), 'aarch64-unknown-linux-gnu');
    assert.equal(getTargetTriple('darwin', 'x64'), 'x86_64-apple-darwin');
    assert.equal(getTargetTriple('darwin', 'arm64'), 'aarch64-apple-darwin');
    assert.throws(() => getTargetTriple('freebsd', 'x64'), /no prebuilt binary/);
    assert.equal(getArchiveExt('win32'), 'zip');
    assert.equal(getArchiveExt('linux'), 'tar.gz');
  });

  test('parseSha256Sums reads sha256sum output', () => {
    const sums = parseSha256Sums(`${'a'.repeat(64)}  one.zip\n${'B'.repeat(64)} *two.tar.gz\n`);
    assert.equal(sums.get('one.zip'), 'a'.repeat(64));
    assert.equal(sums.get('two.tar.gz'), 'b'.repeat(64));
  });

  test('downloads, verifies and installs into the cache', async () => {
    const cache = path.join(tmp, 'cache-ok');
    const env = { FORGE_MCP_CACHE_DIR: cache };
    const installed = await downloadBinary({ version: VERSION, baseUrl, env });
    assert.equal(installed, getCachedBinaryPath({ version: VERSION, env }));
    assert.equal(fs.readFileSync(installed, 'utf8'), STUB);
    if (process.platform !== 'win32') {
      assert.ok(fs.statSync(installed).mode & 0o100, 'binary should be executable');
    }
  });

  test('rejects an archive whose checksum does not match', async () => {
    const sumsPath = path.join(tmp, 'release', TAG, 'SHA256SUMS');
    fs.writeFileSync(sumsPath, `${'0'.repeat(64)}  ${asset}\n`);
    try {
      const env = { FORGE_MCP_CACHE_DIR: path.join(tmp, 'cache-bad') };
      await assert.rejects(downloadBinary({ version: VERSION, baseUrl, env }), /Checksum mismatch/);
      assert.equal(fs.existsSync(getCachedBinaryPath({ version: VERSION, env })), false);
    } finally {
      fs.writeFileSync(sumsPath, goodSums);
    }
  });

  test('resolveBinaryPath downloads into the cache when nothing local exists', (t) => {
    const { resolveBinaryPath, findBinaryInPath, getBinaryName } = require('../index.js');
    // Keep the real PATH: the download step needs `tar` on Linux and macOS.
    const realPath = process.env.PATH || process.env.Path || '';
    if (findBinaryInPath(getBinaryName(), { PATH: realPath })) {
      t.skip('forge-mcp is already on PATH, so resolution would never reach the download step');
      return;
    }
    const empty = path.join(tmp, 'empty-root');
    fs.mkdirSync(empty, { recursive: true });
    const cache = path.join(tmp, 'cache-resolve');
    const env = {
      PATH: realPath,
      FORGE_MCP_CACHE_DIR: cache,
      FORGE_MCP_RELEASE_BASE_URL: baseUrl,
      FORGE_MCP_RELEASE_TAG: TAG
    };
    const chunks = [];
    let resolved;
    try {
      resolved = resolveBinaryPath({
        env,
        version: VERSION,
        packageRoot: empty,
        workspaceRoot: empty,
        autoBuild: false,
        stderr: { write: (c) => chunks.push(String(c)) }
      });
    } catch (err) {
      err.message += `\n--- download output ---\n${chunks.join('')}`;
      throw err;
    }
    assert.equal(resolved, getCachedBinaryPath({ version: VERSION, env }));
    assert.equal(fs.readFileSync(resolved, 'utf8'), STUB);
    assert.match(chunks.join(''), /installed/);

    // A second resolve hits the cache without downloading again.
    chunks.length = 0;
    assert.equal(
      resolveBinaryPath({ env, version: VERSION, packageRoot: empty, workspaceRoot: empty, autoBuild: false, stderr: { write: (c) => chunks.push(String(c)) } }),
      resolved
    );
    assert.equal(chunks.join(''), '');
  });

  test('resolveBinaryPath honors FORGE_MCP_SKIP_DOWNLOAD', () => {
    const { resolveBinaryPath } = require('../index.js');
    const empty = path.join(tmp, 'empty-root-skip');
    fs.mkdirSync(empty, { recursive: true });
    const env = {
      PATH: '',
      FORGE_MCP_CACHE_DIR: path.join(tmp, 'cache-skip'),
      FORGE_MCP_RELEASE_BASE_URL: baseUrl,
      FORGE_MCP_RELEASE_TAG: TAG,
      FORGE_MCP_SKIP_DOWNLOAD: '1'
    };
    assert.throws(
      () => resolveBinaryPath({ env, version: VERSION, packageRoot: empty, workspaceRoot: empty, autoBuild: false, stderr: { write() {} } }),
      /download skipped/
    );
  });

  test('fails clearly when the release does not exist', async () => {
    const env = { FORGE_MCP_CACHE_DIR: path.join(tmp, 'cache-missing') };
    await assert.rejects(
      downloadBinary({ version: '0.0.0-missing', baseUrl, env }),
      /HTTP 404/
    );
  });
});
