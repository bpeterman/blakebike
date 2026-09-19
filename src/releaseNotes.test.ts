import { describe, expect, it } from "vitest";
import {
  formatReleaseDate,
  latestRelease,
  parseChangelog,
  releaseFor,
  releases,
} from "./releaseNotes";

const sample = `# Changelog

Preamble that is not part of any release.

## [Unreleased]

### Added

- Something still cooking.

## [1.2.0] - 2026-01-15

Rides now survive a dropped trainer connection.

### Added

- Automatic reconnect.
- Battery status.

### Fixed

- Stalled ride timer.

## [1.1.0] - 2025-12-01

Heart-rate straps over ANT+.
`;

describe("parseChangelog", () => {
  it("returns released versions newest first and skips Unreleased", () => {
    expect(parseChangelog(sample).map((release) => release.version)).toEqual(["1.2.0", "1.1.0"]);
  });

  it("reads the summary paragraph and grouped items", () => {
    const [latest] = parseChangelog(sample);
    expect(latest.date).toBe("2026-01-15");
    expect(latest.summary).toBe("Rides now survive a dropped trainer connection.");
    expect(latest.sections).toEqual([
      { title: "Added", items: ["Automatic reconnect.", "Battery status."] },
      { title: "Fixed", items: ["Stalled ride timer."] },
    ]);
  });

  it("keeps a release that has only a summary", () => {
    const [, previous] = parseChangelog(sample);
    expect(previous.summary).toBe("Heart-rate straps over ANT+.");
    expect(previous.sections).toEqual([]);
  });

  it("ignores prose outside of a version heading", () => {
    expect(parseChangelog(sample).some((release) => release.summary.includes("Preamble"))).toBe(false);
  });
});

describe("the changelog shipped with the app", () => {
  it("parses into at least one release with a summary", () => {
    expect(releases.length).toBeGreaterThan(0);
    expect(latestRelease?.summary).not.toBe("");
  });

  it("falls back to the latest release for an unknown version", () => {
    expect(releaseFor("99.0.0")).toBe(latestRelease);
    expect(releaseFor(null)).toBe(latestRelease);
  });
});

describe("formatReleaseDate", () => {
  it("formats an ISO date and passes through missing or bad input", () => {
    expect(formatReleaseDate("2026-01-15")).toContain("2026");
    expect(formatReleaseDate(null)).toBeNull();
    expect(formatReleaseDate("nonsense")).toBeNull();
  });
});
