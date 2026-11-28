import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const script = fileURLToPath(new URL('./check-version.mjs', import.meta.url));
const { version } = JSON.parse(readFileSync(new URL('../npm/package.json', import.meta.url)));
const baseVersion = version.split('-')[0];

for (const [name, ref, type, expected] of [
  ['matching release tag', `v${version}`, 'tag', 0],
  ['matching prerelease tag', `v${version.includes('-') ? version : baseVersion + '-rc.1'}`, 'tag', 0],
  ['version-like branch name', 'v1-dev', 'branch', 0],
  ['version mismatch', 'v9999.0.0', 'tag', 1],
  ['malformed release tag', 'vnext', 'tag', 1],
  ['malformed prerelease tag', `v${baseVersion}-`, 'tag', 1],
]) {
  test(name, () => {
    const result = spawnSync(process.execPath, [script], {
      encoding: 'utf8',
      env: { ...process.env, GITHUB_REF_NAME: ref, GITHUB_REF_TYPE: type },
    });
    assert.ifError(result.error);
    assert.equal(result.status, expected, result.stderr);
    assert.match(expected ? result.stderr : result.stdout, /^check-version:/);
  });
}
