#!/usr/bin/env node
// Verifies that Cargo.toml, npm/package.json and (when present) the release tag
// all carry the same version. Exits non-zero on any mismatch.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const cargoToml = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const cargoMatch = /^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m.exec(cargoToml);
if (!cargoMatch) {
  console.error('check-version: could not find [package].version in Cargo.toml');
  process.exit(1);
}
const cargoVersion = cargoMatch[1];

const pkg = JSON.parse(fs.readFileSync(path.join(root, 'npm', 'package.json'), 'utf8'));
const npmVersion = pkg.version;

const problems = [];
if (cargoVersion !== npmVersion) {
  problems.push(`Cargo.toml version ${cargoVersion} != npm/package.json version ${npmVersion}`);
}

const ref = process.env.GITHUB_REF_NAME || '';
const refType = process.env.GITHUB_REF_TYPE;
if (refType === 'tag' || (!refType && /^v\d/.test(ref))) {
  const tag = /^v(\d+\.\d+\.\d+)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.exec(ref);
  if (!tag) {
    problems.push('release tag must have the form vX.Y.Z or vX.Y.Z-rc.N');
  } else if (ref.slice(1) !== cargoVersion && tag[1] !== cargoVersion) {
    // A prerelease can rehearse a stable version without changing the manifests.
    problems.push(`tag ${ref} does not match Cargo.toml version ${cargoVersion}`);
  }
}

if (problems.length > 0) {
  for (const p of problems) console.error(`check-version: ${p}`);
  process.exit(1);
}

console.log(`check-version: ok (${cargoVersion})`);
