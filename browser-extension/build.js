#!/usr/bin/env node
// build.js — assemble dist/chrome/ and dist/firefox/ from the shared src/,
// vendor/, icons/ plus the browser-specific manifest.
//
// Also concatenates vendor/marked.min.js + vendor/highlight.min.js into
// dist/<target>/src/vendor-bundle.js so sidepanel.html can load them as a
// single same-directory <script> without any path-resolution ambiguity.
//
// Usage:
//   node build.js chrome
//   node build.js firefox
//   node build.js            (builds both)

const fs = require('fs');
const path = require('path');

const ROOT = __dirname;
const TARGETS = {
  chrome: 'manifest.chrome.json',
  firefox: 'manifest.firefox.json',
};

// Vendor libs bundled into vendor-bundle.js (loaded as a plain <script> in sidepanel.html)
const VENDOR_BUNDLE = [
  'vendor/marked.min.js',
  'vendor/highlight.min.js',
  'vendor/katex.min.js',
];

function copyRecursive(src, dest) {
  const stat = fs.statSync(src);
  if (stat.isDirectory()) {
    fs.mkdirSync(dest, { recursive: true });
    for (const entry of fs.readdirSync(src)) {
      copyRecursive(path.join(src, entry), path.join(dest, entry));
    }
  } else {
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    fs.copyFileSync(src, dest);
  }
}

function buildTarget(name) {
  const manifestFile = TARGETS[name];
  if (!manifestFile) {
    throw new Error(`unknown target: ${name} (expected one of ${Object.keys(TARGETS).join(', ')})`);
  }

  const outDir = path.join(ROOT, 'dist', name);
  fs.rmSync(outDir, { recursive: true, force: true });
  fs.mkdirSync(outDir, { recursive: true });

  copyRecursive(path.join(ROOT, 'src'), path.join(outDir, 'src'));
  copyRecursive(path.join(ROOT, 'vendor'), path.join(outDir, 'vendor'));
  if (fs.existsSync(path.join(ROOT, 'icons'))) {
    copyRecursive(path.join(ROOT, 'icons'), path.join(outDir, 'icons'));
  }
  fs.copyFileSync(path.join(ROOT, manifestFile), path.join(outDir, 'manifest.json'));

  // Write vendor libs as a single external file in src/ so sidepanel.html can
  // load it with <script src="vendor-bundle.js"> (same directory, no path
  // ambiguity). Chrome MV3 CSP allows script-src 'self' for external files
  // but blocks inline <script> blocks entirely.
  const vendorJs = VENDOR_BUNDLE.map(rel => fs.readFileSync(path.join(ROOT, rel), 'utf8')).join('\n');
  fs.writeFileSync(path.join(outDir, 'src', 'vendor-bundle.js'), vendorJs, 'utf8');

  console.log(`[build] ${name} -> ${path.relative(ROOT, outDir)}/`);
}

const args = process.argv.slice(2);
const targets = args.length ? args : Object.keys(TARGETS);
for (const t of targets) buildTarget(t);
