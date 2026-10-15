import changelogSource from "../CHANGELOG.md?raw";

export type ReleaseSection = {
  title: string;
  items: string[];
};

export type Release = {
  version: string;
  date: string | null;
  /** One-sentence rider-facing summary, shown on the About card. */
  summary: string;
  sections: ReleaseSection[];
};

const versionHeading = /^##\s+\[(\d+\.\d+\.\d+)\](?:\s+-\s+(\d{4}-\d{2}-\d{2}))?\s*$/;
const sectionHeading = /^###\s+(.+?)\s*$/;
const listItem = /^[-*]\s+(.+?)\s*$/;

/**
 * Parses the Keep a Changelog file that ships with the app. Released versions
 * only: `## [Unreleased]` and anything that is not a semver heading is ignored,
 * so work in progress never reaches the About card.
 */
export function parseChangelog(source: string): Release[] {
  const releases: Release[] = [];
  let current: Release | null = null;
  let section: ReleaseSection | null = null;
  let summaryDone = false;

  for (const rawLine of source.split("\n")) {
    const line = rawLine.trimEnd();

    const version = versionHeading.exec(line);
    if (version) {
      current = { version: version[1], date: version[2] ?? null, summary: "", sections: [] };
      releases.push(current);
      section = null;
      summaryDone = false;
      continue;
    }
    if (line.startsWith("## ")) {
      current = null;
      section = null;
      continue;
    }
    if (!current) continue;

    const heading = sectionHeading.exec(line);
    if (heading) {
      section = { title: heading[1], items: [] };
      current.sections.push(section);
      summaryDone = true;
      continue;
    }

    const item = listItem.exec(line);
    if (item) {
      if (section) section.items.push(item[1]);
      continue;
    }

    if (line.trim() === "") {
      if (current.summary) summaryDone = true;
      continue;
    }
    if (!summaryDone && !section) {
      current.summary = current.summary ? `${current.summary} ${line.trim()}` : line.trim();
    }
  }

  return releases.filter((release) => release.sections.length > 0 || release.summary !== "");
}

export const releases = parseChangelog(changelogSource);
export const latestRelease: Release | null = releases[0] ?? null;

export function releaseFor(version: string | null): Release | null {
  if (!version) return latestRelease;
  return releases.find((release) => release.version === version) ?? latestRelease;
}

export function formatReleaseDate(date: string | null): string | null {
  if (!date) return null;
  const parsed = new Date(`${date}T00:00:00`);
  if (Number.isNaN(parsed.getTime())) return null;
  return parsed.toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" });
}
