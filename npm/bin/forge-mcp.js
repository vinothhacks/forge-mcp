#!/usr/bin/env node
'use strict';

const path = require('node:path');
const { resolveBinaryPath, spawnForgeServer } = require('../index.js');

/**
 * CLI entrypoint for `forge-codebase-mcp`.
 * Resolves (or builds) the native `forge-mcp` binary and spawns it,
 * forwarding all CLI arguments, environment variables, stdio streams,
 * and termination signals without polluting stdout.
 */
function main() {
  const args = process.argv.slice(2);
  const packageRoot = path.resolve(__dirname, '..');
  const workspaceRoot = path.resolve(packageRoot, '..');

  let binPath;
  try {
    binPath = resolveBinaryPath({
      env: process.env,
      packageRoot,
      workspaceRoot,
      autoBuild: true,
      stderr: process.stderr
    });
  } catch (err) {
    process.stderr.write(`[forge-mcp] ${err.message}\n`);
    process.exit(1);
  }

  const child = spawnForgeServer(args, {
    binPath,
    cwd: process.cwd(),
    env: process.env,
    stdio: ['pipe', 'pipe', 'pipe'],
    forwardSignals: true
  });

  if (child.stdin) {
    child.stdin.on('error', () => {
      // Ignore EPIPE / EOF when child closes stdin early
    });
    process.stdin.on('error', () => {
      // Ignore stdin stream errors
    });
    process.stdin.pipe(child.stdin);
  }

  if (child.stdout) {
    child.stdout.on('error', () => {});
    process.stdout.on('error', () => {});
    child.stdout.pipe(process.stdout);
  }

  if (child.stderr) {
    child.stderr.on('error', () => {});
    process.stderr.on('error', () => {});
    child.stderr.pipe(process.stderr);
  }

  process.on('exit', () => {
    if (child.exitCode === null && !child.killed) {
      try {
        child.kill('SIGTERM');
      } catch (_) {
        // Ignore if already exited
      }
    }
  });

  child.on('error', (err) => {
    process.stderr.write(`[forge-mcp] Failed to start binary (${binPath}): ${err.message}\n`);
    process.exit(1);
  });

  child.on('exit', (code, signal) => {
    if (signal) {
      process.exit(1);
    }
    process.exit(code ?? 0);
  });
}

if (require.main === module) {
  main();
}

module.exports = { main };
