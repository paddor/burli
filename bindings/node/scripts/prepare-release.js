#!/usr/bin/env node
"use strict";

// Sets the release version on the root and platform packages, and makes the
// platform packages optional dependencies of the root one.

const fs = require("node:fs");
const path = require("node:path");

const args = process.argv.slice(2);
const dryRun = args.includes("--dry-run");
const version = args.find((arg) => !arg.startsWith("--"));

if (!version || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
  throw new Error("usage: node scripts/prepare-release.js VERSION [--dry-run]");
}

const rootDir = path.resolve(__dirname, "..");
const readJson = (file) => JSON.parse(fs.readFileSync(file, "utf8"));
const writeJson = (file, value) => {
  if (!dryRun) {
    fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
  }
};

const rootPath = path.join(rootDir, "package.json");
const root = readJson(rootPath);
const platforms = fs.readdirSync(path.join(rootDir, "npm")).sort();
const optionalDependencies = {};

for (const platform of platforms) {
  const packagePath = path.join(rootDir, "npm", platform, "package.json");
  const platformPackage = readJson(packagePath);
  platformPackage.version = version;
  optionalDependencies[platformPackage.name] = version;
  writeJson(packagePath, platformPackage);
}

root.version = version;
root.optionalDependencies = optionalDependencies;
writeJson(rootPath, root);

console.log(`${dryRun ? "would prepare" : "prepared"} ${root.name}@${version} with ${platforms.length} platform packages`);
