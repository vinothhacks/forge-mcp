'use strict';

const os = require('node:os');
const path = require('node:path');

/** GitHub repository that hosts the release binaries. */
const RELEASE_REPO = 'vinothhacks/forge-mcp';
const DEFAULT_RELEASE_BASE_URL = `https://github.com/${RELEASE_REPO}/releases/download`;

const TARGETS = {
  'win32-x64': 'x86_64-pc-windows-msvc',
  'linux-x64': 'x86_64-unknown-linux-gnu',
  'linux-arm64': 'aarch64-unknown-linux-gnu',
  'darwin-x64': 'x86_64-apple-darwin',
  'darwin-arm64': 'aarch64-apple-darwin'
};

/**
 * Maps a Node.js platform/arch pair to the Rust target triple used in release asset names.
 * @param {string} [platform=process.platform]
 * @param {string} [arch=process.arch]
 * @returns {string}
 */
function getTargetTriple(platform = process.platform, arch = process.arch) {
  const triple = TARGETS[`${platform}-${arch}`];
  if (!triple) {
    throw new Error(
      `forge-mcp has no prebuilt binary for ${platform}/${arch}. ` +
        'Install from source with "cargo install forge-mcp --locked" and set FORGE_MCP_BIN, or put forge-mcp on PATH.'
    );
  }
  return triple;
}

/**
 * Archive extension used for a platform's release asset.
 * @param {string} [platform=process.platform]
 * @returns {'zip'|'tar.gz'}
 */
function getArchiveExt(platform = process.platform) {
  return platform === 'win32' ? 'zip' : 'tar.gz';
}

/**
 * Release asset file name, e.g. `forge-mcp-v0.1.0-x86_64-pc-windows-msvc.zip`.
 * @param {string} version Version without leading `v`
 * @param {string} triple
 * @param {string} ext
 * @returns {string}
 */
function getAssetName(version, triple, ext) {
  return `forge-mcp-v${version}-${triple}.${ext}`;
}

/**
 * Root cache directory for downloaded binaries.
 * @param {NodeJS.ProcessEnv} [env=process.env]
 * @returns {string}
 */
function getCacheDir(env = process.env) {
  const override = env.FORGE_MCP_CACHE_DIR && String(env.FORGE_MCP_CACHE_DIR).trim();
  return override ? path.resolve(override) : path.join(os.homedir(), '.forge-mcp');
}

/**
 * Path where the downloaded binary for a version/triple is stored.
 * @param {object} opts
 * @param {string} opts.version
 * @param {string} [opts.triple]
 * @param {string} [opts.platform]
 * @param {NodeJS.ProcessEnv} [opts.env]
 * @returns {string}
 */
function getCachedBinaryPath({ version, triple, platform = process.platform, env = process.env }) {
  const t = triple || getTargetTriple(platform);
  const bin = platform === 'win32' ? 'forge-mcp.exe' : 'forge-mcp';
  return path.join(getCacheDir(env), 'bin', version, t, bin);
}

/**
 * The `tar` executable to use. On Windows this is the built-in bsdtar in System32,
 * because a GNU tar earlier on PATH (Git Bash, MSYS2) reads `C:` as a remote host
 * and cannot handle zip archives.
 * @param {string} [platform=process.platform]
 * @param {NodeJS.ProcessEnv} [env=process.env]
 * @returns {string}
 */
function getTarCommand(platform = process.platform, env = process.env) {
  if (platform === 'win32') {
    const systemRoot = env.SystemRoot || env.SYSTEMROOT || 'C:\\Windows';
    const bsdtar = path.join(systemRoot, 'System32', 'tar.exe');
    if (require('node:fs').existsSync(bsdtar)) return bsdtar;
  }
  return 'tar';
}

module.exports = {
  getTarCommand,
  RELEASE_REPO,
  DEFAULT_RELEASE_BASE_URL,
  getTargetTriple,
  getArchiveExt,
  getAssetName,
  getCacheDir,
  getCachedBinaryPath
};
