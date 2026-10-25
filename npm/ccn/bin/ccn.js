#!/usr/bin/env node
// Runs the native ccn binary from the platform package npm installed beside this one.
// Under npx that package lives in a cache that can be cleared, so CCN_EPHEMERAL tells
// `ccn pair` / `ccn setup` to copy the binary somewhere permanent before writing the hooks.
'use strict';
const { spawnSync } = require('child_process');
const path = require('path');

const pkg = `@touchtap/ccn-${process.platform}-${process.arch}`;
const exe = process.platform === 'win32' ? 'ccn.exe' : 'ccn';
let binary;
try {
  binary = require.resolve(`${pkg}/bin/${exe}`);
} catch {
  console.error(`ccn: no build for ${process.platform}-${process.arch} (looked for ${pkg}).`);
  console.error('See https://claudecodenotifier.com/cli for other ways to install.');
  process.exit(1);
}

const ephemeral = binary.split(path.sep).includes('_npx');
const result = spawnSync(binary, process.argv.slice(2), {
  stdio: 'inherit',
  env: ephemeral ? { ...process.env, CCN_EPHEMERAL: '1' } : process.env,
});
if (result.error) {
  console.error(`ccn: ${result.error.message}`);
  process.exit(1);
}
process.exit(result.status ?? 1);
