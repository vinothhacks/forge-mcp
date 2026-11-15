#!/usr/bin/env node
'use strict';

/**
 * Downloads the forge-mcp release binary for this platform from GitHub Releases,
 * verifies it against the release SHA256SUMS file, and installs it into the cache.
 *
 * Runnable as a script so the synchronous `resolveBinaryPath()` can invoke it with
 * `spawnSync`. All progress output goes to stderr; stdout stays empty so a stdio
 * MCP session is never polluted.
 *
 *   node download.js --version 0.1.0 [--triple <t>] [--dest <file>] [--base-url <url>] [--tag <tag>]
 *
 * Environment overrides: FORGE_MCP_RELEASE_BASE_URL, FORGE_MCP_RELEASE_TAG, FORGE_MCP_CACHE_DIR.
 */

const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const { spawnSync } = require('node:child_process');
const {
  DEFAULT_RELEASE_BASE_URL,
  getTargetTriple,
  getArchiveExt,
  getAssetName,
  getCachedBinaryPath,
  getTarCommand
} = require('./platform.js');

function log(msg) {
  process.stderr.write(`[forge-mcp] ${msg}\n`);
}

async function fetchBuffer(url) {
  const res = await fetch(url, { redirect: 'follow' });
  if (!res.ok) {
    throw new Error(`GET ${url} failed: HTTP ${res.status} ${res.statusText}`);
  }
  return Buffer.from(await res.arrayBuffer());
}

/**
 * Parses a `sha256sum`-style file into a Map of file name -> lowercase hex digest.
 * @param {string} text
 * @returns {Map<string, string>}
 */
function parseSha256Sums(text) {
  const out = new Map();
  for (const line of text.split(/\r?\n/)) {
    const m = /^([0-9a-fA-F]{64})\s+\*?(.+)$/.exec(line.trim());
    if (m) out.set(path.basename(m[2].trim()), m[1].toLowerCase());
  }
  return out;
}

function extractArchive(archivePath, destDir, ext) {
  const tarArgs = ext === 'zip' ? ['-xf', archivePath, '-C', destDir] : ['-xzf', archivePath, '-C', destDir];
  const tar = spawnSync(getTarCommand(), tarArgs, { stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
  if (tar.status === 0) return;
  if (ext === 'zip' && process.platform === 'win32') {
    const ps = spawnSync(
      'powershell',
      [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        `Expand-Archive -LiteralPath '${archivePath.replace(/'/g, "''")}' -DestinationPath '${destDir.replace(/'/g, "''")}' -Force`
      ],
      { stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true }
    );
    if (ps.status === 0) return;
    throw new Error(`Failed to extract ${archivePath}: ${String(ps.stderr || '').trim()}`);
  }
  throw new Error(`Failed to extract ${archivePath} with tar: ${String(tar.stderr || tar.error || '').trim()}`);
}

function findFile(dir, name) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      const hit = findFile(full, name);
      if (hit) return hit;
    } else if (entry.name === name) {
      return full;
    }
  }
  return null;
}

/**
 * Downloads, verifies and installs the release binary.
 * @param {object} opts
 * @param {string} opts.version Package version without leading `v`
 * @param {string} [opts.triple]
 * @param {string} [opts.dest] Final binary path (defaults to the cache path)
 * @param {string} [opts.baseUrl]
 * @param {string} [opts.tag]
 * @param {string} [opts.platform]
 * @param {NodeJS.ProcessEnv} [opts.env]
 * @returns {Promise<string>} Installed binary path
 */
async function downloadBinary(opts) {
  const env = opts.env || process.env;
  const platform = opts.platform || process.platform;
  const version = opts.version;
  if (!version) throw new Error('downloadBinary: version is required');
  const triple = opts.triple || getTargetTriple(platform);
  const ext = getArchiveExt(platform);
  const tag = opts.tag || env.FORGE_MCP_RELEASE_TAG || `v${version}`;
  const assetVersion = tag.replace(/^v/, '');
  const baseUrl = (opts.baseUrl || env.FORGE_MCP_RELEASE_BASE_URL || DEFAULT_RELEASE_BASE_URL).replace(/\/+$/, '');
  const dest = opts.dest || getCachedBinaryPath({ version, triple, platform, env });
  const asset = getAssetName(assetVersion, triple, ext);
  const binName = platform === 'win32' ? 'forge-mcp.exe' : 'forge-mcp';

  log(`downloading ${asset} from ${baseUrl}/${tag}/`);
  const sums = parseSha256Sums((await fetchBuffer(`${baseUrl}/${tag}/SHA256SUMS`)).toString('utf8'));
  const expected = sums.get(asset);
  if (!expected) throw new Error(`SHA256SUMS for ${tag} has no entry for ${asset}`);

  const archive = await fetchBuffer(`${baseUrl}/${tag}/${asset}`);
  const actual = crypto.createHash('sha256').update(archive).digest('hex');
  if (actual !== expected) {
    throw new Error(`Checksum mismatch for ${asset}: expected ${expected}, got ${actual}`);
  }

  fs.mkdirSync(path.dirname(dest), { recursive: true });
  const work = fs.mkdtempSync(path.join(os.tmpdir(), 'forge-mcp-dl-'));
  try {
    const archivePath = path.join(work, asset);
    fs.writeFileSync(archivePath, archive);
    const extractDir = path.join(work, 'x');
    fs.mkdirSync(extractDir);
    extractArchive(archivePath, extractDir, ext);
    const extracted = findFile(extractDir, binName);
    if (!extracted) throw new Error(`${asset} does not contain ${binName}`);
    const staged = `${dest}.tmp-${process.pid}`;
    fs.copyFileSync(extracted, staged);
    if (platform !== 'win32') fs.chmodSync(staged, 0o755);
    fs.renameSync(staged, dest);
  } finally {
    fs.rmSync(work, { recursive: true, force: true });
  }
  log(`installed ${dest}`);
  return dest;
}

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 1) {
    const key = argv[i];
    const val = argv[i + 1];
    if (key === '--version') out.version = val;
    else if (key === '--triple') out.triple = val;
    else if (key === '--dest') out.dest = val;
    else if (key === '--base-url') out.baseUrl = val;
    else if (key === '--tag') out.tag = val;
    else continue;
    i += 1;
  }
  return out;
}

async function main(argv = process.argv.slice(2)) {
  const opts = parseArgs(argv);
  if (!opts.version) {
    opts.version = require('../package.json').version;
  }
  try {
    await downloadBinary(opts);
    return 0;
  } catch (err) {
    log(err.message);
    return 1;
  }
}

if (require.main === module) {
  main().then((code) => process.exit(code));
}

module.exports = { downloadBinary, parseSha256Sums, main };
