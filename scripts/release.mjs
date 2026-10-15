#!/usr/bin/env node
// Release plumbing for blake.bike.
//
//   node scripts/release.mjs prepare <patch|minor|major|X.Y.Z>
//       Cuts `## [Unreleased]` into a dated version heading and bumps the
//       version everywhere. Leaves the changes uncommitted for review.
//   node scripts/release.mjs notes [version]
//       Prints the GitHub release body for a version (defaults to the current).
//   node scripts/release.mjs check [--tag vX.Y.Z]
//       Fails if the version, the manifests, and the changelog disagree.
//
// CHANGELOG.md is the single source of release notes: the same text reaches the
// GitHub release and the About card in the app.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import process from "node:process";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const paths = {
  changelog: join(root, "CHANGELOG.md"),
  packageJson: join(root, "package.json"),
  tauriConf: join(root, "src-tauri", "tauri.conf.json"),
  cargoToml: join(root, "src-tauri", "Cargo.toml"),
  cargoLock: join(root, "src-tauri", "Cargo.lock"),
};

const read = (path) => readFileSync(path, "utf8");
const fail = (message) => {
  console.error(`error: ${message}`);
  process.exit(1);
};

const semver = /^\d+\.\d+\.\d+$/;

function packageVersion() {
  return JSON.parse(read(paths.packageJson)).version;
}

function tauriConfVersion() {
  return JSON.parse(read(paths.tauriConf)).version ?? null;
}

function cargoVersion() {
  const match = /^version = "(.+?)"$/m.exec(read(paths.cargoToml));
  return match?.[1] ?? null;
}

function cargoLockVersion() {
  const match = /name = "blakebike"\nversion = "(.+?)"/.exec(read(paths.cargoLock));
  return match?.[1] ?? null;
}

/** Releases in file order, newest first. `## [Unreleased]` is not a release. */
function parseChangelog(source = read(paths.changelog)) {
  const lines = source.split("\n");
  const releases = [];
  let current = null;
  for (const line of lines) {
    const heading = /^##\s+\[(\d+\.\d+\.\d+)\](?:\s+-\s+(\d{4}-\d{2}-\d{2}))?\s*$/.exec(line);
    if (heading) {
      current = { version: heading[1], date: heading[2] ?? null, body: [] };
      releases.push(current);
      continue;
    }
    if (line.startsWith("## ")) {
      current = null;
      continue;
    }
    current?.body.push(line);
  }
  return releases.map((release) => ({
    ...release,
    body: release.body.join("\n").trim(),
    summary: firstParagraph(release.body),
  }));
}

function firstParagraph(lines) {
  const collected = [];
  for (const line of lines) {
    if (line.startsWith("###")) break;
    if (line.trim() === "") {
      if (collected.length > 0) break;
      continue;
    }
    collected.push(line.trim());
  }
  return collected.join(" ");
}

function unreleasedBody(source = read(paths.changelog)) {
  const match = /^## \[Unreleased\]\s*$/m.exec(source);
  if (!match) fail("CHANGELOG.md has no `## [Unreleased]` section.");
  const after = source.slice(match.index + match[0].length);
  const next = /^## /m.exec(after);
  return (next ? after.slice(0, next.index) : after).trim();
}

function nextVersion(current, bump) {
  if (semver.test(bump)) return bump;
  const [major, minor, patch] = current.split(".").map(Number);
  if (bump === "major") return `${major + 1}.0.0`;
  if (bump === "minor") return `${major}.${minor + 1}.0`;
  if (bump === "patch") return `${major}.${minor}.${patch + 1}`;
  return fail(`unknown bump "${bump}". Use major, minor, patch, or an explicit X.Y.Z.`);
}

function today() {
  return new Date().toISOString().slice(0, 10);
}

function prepare(bump) {
  if (!bump) fail("usage: pnpm release <patch|minor|major|X.Y.Z>");
  const current = packageVersion();
  const version = nextVersion(current, bump);
  const source = read(paths.changelog);

  if (parseChangelog(source).some((release) => release.version === version)) {
    fail(`CHANGELOG.md already has an entry for ${version}.`);
  }
  const pending = unreleasedBody(source);
  if (pending === "") {
    fail("`## [Unreleased]` is empty. Write the notes for this release first.");
  }
  if (pending.startsWith("###")) {
    fail(
      "`## [Unreleased]` needs a one-sentence summary paragraph before the first\n" +
        "`###` section. That sentence is what riders see on the About card.",
    );
  }

  const heading = /^## \[Unreleased\]\s*$/m.exec(source);
  const rest = source.slice(heading.index + heading[0].length).replace(/^\n+/, "");
  const updated =
    source.slice(0, heading.index) +
    `## [Unreleased]\n\n## [${version}] - ${today()}\n\n` +
    rest;
  writeFileSync(paths.changelog, updated);

  writeFileSync(
    paths.packageJson,
    read(paths.packageJson).replace(`"version": "${current}"`, `"version": "${version}"`),
  );
  writeFileSync(
    paths.tauriConf,
    read(paths.tauriConf).replace(`"version": "${current}"`, `"version": "${version}"`),
  );
  writeFileSync(
    paths.cargoToml,
    read(paths.cargoToml).replace(`version = "${current}"`, `version = "${version}"`),
  );
  writeFileSync(
    paths.cargoLock,
    read(paths.cargoLock).replace(
      `name = "blakebike"\nversion = "${current}"`,
      `name = "blakebike"\nversion = "${version}"`,
    ),
  );

  check([]);
  console.log(`Prepared ${current} -> ${version}. Nothing has been committed.\n`);
  console.log("Review, then:\n");
  console.log("  git diff");
  console.log("  git add CHANGELOG.md package.json src-tauri/tauri.conf.json \\");
  console.log("    src-tauri/Cargo.toml src-tauri/Cargo.lock");
  console.log(`  git commit -m "Release v${version}"`);
  console.log(`  git tag v${version}`);
  console.log("  git push --follow-tags\n");
  console.log("The tag push builds the dmg and publishes the GitHub release.");
}

function notes(requested) {
  const version = requested?.replace(/^v/, "") ?? packageVersion();
  const release = parseChangelog().find((entry) => entry.version === version);
  if (!release) fail(`CHANGELOG.md has no entry for ${version}.`);
  console.log(release.body);
}

function check(args) {
  const version = packageVersion();
  const releases = parseChangelog();
  const latest = releases[0];
  const problems = [];

  if (!semver.test(version)) problems.push(`package.json version "${version}" is not semver.`);
  if (tauriConfVersion() !== version) {
    problems.push(
      `src-tauri/tauri.conf.json is ${tauriConfVersion()}, package.json is ${version}.`,
    );
  }
  if (cargoVersion() !== version) {
    problems.push(`src-tauri/Cargo.toml is ${cargoVersion()}, package.json is ${version}.`);
  }
  if (cargoLockVersion() !== version) {
    problems.push(`src-tauri/Cargo.lock is ${cargoLockVersion()}, package.json is ${version}.`);
  }
  if (!latest) {
    problems.push("CHANGELOG.md has no released versions.");
  } else if (latest.version !== version) {
    problems.push(
      `CHANGELOG.md's newest release is ${latest.version}, package.json is ${version}. ` +
        "Run `pnpm release` to cut a version instead of editing the files by hand.",
    );
  } else {
    if (!latest.date) problems.push(`CHANGELOG.md entry for ${version} has no date.`);
    if (!latest.summary) {
      problems.push(
        `CHANGELOG.md entry for ${version} has no summary paragraph. The About card needs one sentence.`,
      );
    }
  }

  const tagIndex = args.indexOf("--tag");
  if (tagIndex !== -1) {
    const tag = args[tagIndex + 1];
    if (tag !== `v${version}`) problems.push(`tag ${tag} does not match version ${version}.`);
  }

  const seen = new Set();
  for (const release of releases) {
    if (seen.has(release.version)) problems.push(`CHANGELOG.md lists ${release.version} twice.`);
    seen.add(release.version);
  }

  if (problems.length > 0) {
    for (const problem of problems) console.error(`error: ${problem}`);
    process.exit(1);
  }
  console.log(
    `version ${version} is consistent across package.json, tauri.conf.json, Cargo, and CHANGELOG.md`,
  );
}

const [command, ...rest] = process.argv.slice(2);
if (command === "prepare") prepare(rest[0]);
else if (command === "notes") notes(rest[0]);
else if (command === "check") check(rest);
else fail("usage: release.mjs <prepare|notes|check>");
