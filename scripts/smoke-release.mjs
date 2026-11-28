import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const [archiveBinary, tag] = process.argv.slice(2);
assert(archiveBinary && tag?.startsWith('v'), 'Usage: node scripts/smoke-release.mjs <binary> <tag>');
const binary = resolve(archiveBinary);
const version = tag.slice(1);
const dbDir = mkdtempSync(join(tmpdir(), 'forge-release-smoke-'));
const protocolVersion = '2024-11-05';
const request = {
  jsonrpc: '2.0',
  id: 1,
  method: 'initialize',
  params: { protocolVersion, capabilities: {}, clientInfo: { name: 'release-smoke', version: '1' } },
};

function run(args, input) {
  const result = spawnSync(binary, args, { input, encoding: 'utf8', timeout: 10_000 });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

try {
  const cliVersion = run(['--version']);
  const binaryVersion = cliVersion.replace(/^forge-mcp /, '');
  assert(cliVersion.startsWith('forge-mcp '), `Unexpected CLI version: ${cliVersion}`);
  assert(
    binaryVersion === version || binaryVersion === version.split('-')[0],
    `Binary version ${binaryVersion} does not match ${tag}`,
  );

  const response = JSON.parse(run([
    '--transport', 'stdio', '--db-path', join(dbDir, 'index.db'), '--allow-root', dbDir,
  ], JSON.stringify(request) + '\n'));
  assert.equal(response.jsonrpc, '2.0');
  assert.equal(response.id, request.id);
  assert.equal(response.result?.serverInfo.name, 'forge-mcp');
  assert.equal(response.result?.serverInfo.version, binaryVersion);
  assert.equal(response.result?.protocolVersion, protocolVersion);
  console.log(`${cliVersion}; stdio initialize OK (${protocolVersion})`);
} finally {
  rmSync(dbDir, { recursive: true, force: true });
}
