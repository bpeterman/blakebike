import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WorkoutProfile } from "./WorkoutProfile";
import { derivedPowerZones, type WorkoutStep } from "./types";
import { zoneColors } from "./zones";

const FTP = 200;
const zones = derivedPowerZones(FTP);

const steady = (durationSeconds: number, value: number): WorkoutStep => ({
  kind: "steady",
  durationSeconds,
  target: { unit: "percentFtp", value },
});

const steps: WorkoutStep[] = [
  steady(300, 60),
  { kind: "ramp", durationSeconds: 300, start: { unit: "percentFtp", value: 50 }, end: { unit: "percentFtp", value: 110 } },
  { kind: "repeat", repetitions: 2, steps: [steady(60, 120), { kind: "freeRide", durationSeconds: 60 }] },
];

const polygons = (container: HTMLElement) =>
  [...container.querySelectorAll<SVGPolygonElement>("polygon[data-path]")];

afterEach(cleanup);

describe("WorkoutProfile", () => {
  it("draws one block per executed step, coloured by zone, with ramps as gradients", () => {
    const { container } = render(
      <WorkoutProfile steps={steps} ftpWatts={FTP} powerZones={zones} variant="editor" />,
    );
    const blocks = polygons(container);
    expect(blocks).toHaveLength(6);
    expect(blocks.map((block) => block.dataset.path)).toEqual(["0", "1", "2.0", "2.1", "2.0", "2.1"]);
    // 60 % of 200 W = 120 W sits in zone 2 (111–150 W).
    expect(blocks[0].getAttribute("fill")).toBe(zoneColors[1]);
    expect(blocks[1].getAttribute("fill")).toMatch(/^url\(#.*-ramp-1\)$/);
    expect(container.querySelector("linearGradient stop")).toHaveAttribute("stop-color", zoneColors[0]);
    expect(blocks[3].dataset.kind).toBe("freeRide");
    expect(blocks[3].getAttribute("fill")).toMatch(/^url\(#.*-hatch\)$/);
  });

  it("describes the workout for assistive technology", () => {
    render(<WorkoutProfile steps={steps} ftpWatts={FTP} powerZones={zones} />);
    expect(screen.getByRole("img", { name: "14:00 workout, 6 blocks, peak 120% FTP, 2:00 free ride" })).toBeInTheDocument();
  });

  it("shows the FTP line, time axis, and repeat brackets only in the editor variant", () => {
    const editor = render(
      <WorkoutProfile steps={steps} ftpWatts={FTP} powerZones={zones} variant="editor" />,
    );
    expect(editor.container.querySelector("[data-ftp-line]")).toBeInTheDocument();
    expect(editor.container.querySelectorAll(".workout-profile-tick-label").length).toBeGreaterThan(0);
    expect(editor.container.querySelector("[data-repeat='2'] text")).toHaveTextContent("2×");
    cleanup();

    const compact = render(
      <WorkoutProfile steps={steps} ftpWatts={FTP} powerZones={zones} variant="compact" />,
    );
    expect(compact.container.querySelector("[data-ftp-line]")).not.toBeInTheDocument();
    expect(compact.container.querySelectorAll(".workout-profile-tick-label")).toHaveLength(0);
    expect(compact.container.querySelector("[data-repeat]")).not.toBeInTheDocument();
    expect(polygons(compact.container)).toHaveLength(6);
  });

  it("moves the FTP line when FTP changes", () => {
    const { container, rerender } = render(
      <WorkoutProfile steps={[steady(600, 150)]} ftpWatts={200} powerZones={zones} variant="editor" />,
    );
    const before = Number(container.querySelector("[data-ftp-line]")?.getAttribute("y1"));
    rerender(
      <WorkoutProfile steps={[steady(600, 150)]} ftpWatts={250} powerZones={derivedPowerZones(250)} variant="editor" />,
    );
    const after = Number(container.querySelector("[data-ftp-line]")?.getAttribute("y1"));
    // Peak is 150 % of FTP either way, so the axis top is the peak and the FTP line sits at 2/3 height.
    expect(before).toBeCloseTo(after, 5);
    expect(before).toBeGreaterThan(0);
  });

  it("renders the empty frame with a message in the editor and nothing when compact", () => {
    const editor = render(
      <WorkoutProfile steps={[]} ftpWatts={FTP} powerZones={zones} variant="editor" emptyMessage="Nothing yet" />,
    );
    expect(screen.getByText("Nothing yet")).toBeInTheDocument();
    expect(editor.container.querySelector("svg")).not.toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Empty workout" })).toBeInTheDocument();
    cleanup();

    const compact = render(<WorkoutProfile steps={[]} ftpWatts={FTP} powerZones={zones} />);
    expect(compact.container.querySelector("svg")).not.toBeInTheDocument();
    expect(compact.container.querySelector(".workout-profile-empty")).not.toBeInTheDocument();
  });

  it("highlights every block inside the highlighted path and dims the rest", () => {
    const { container } = render(
      <WorkoutProfile steps={steps} ftpWatts={FTP} powerZones={zones} highlightedPath={[2]} />,
    );
    const blocks = polygons(container);
    expect(blocks.map((block) => block.dataset.highlighted)).toEqual([
      undefined, undefined, "true", "true", "true", "true",
    ]);
    expect(blocks.map((block) => block.dataset.dimmed)).toEqual([
      "true", "true", undefined, undefined, undefined, undefined,
    ]);
  });

  it("reports hover and selection by step path", () => {
    const onHighlightPath = vi.fn();
    const onSelectPath = vi.fn();
    const { container } = render(
      <WorkoutProfile
        steps={steps}
        ftpWatts={FTP}
        powerZones={zones}
        onHighlightPath={onHighlightPath}
        onSelectPath={onSelectPath}
      />,
    );
    expect(container.querySelector(".workout-profile")).toHaveAttribute("data-interactive", "true");
    const blocks = polygons(container);
    fireEvent.mouseEnter(blocks[4]);
    expect(onHighlightPath).toHaveBeenLastCalledWith([2, 0]);
    fireEvent.click(blocks[1]);
    expect(onSelectPath).toHaveBeenCalledWith([1]);
    fireEvent.mouseLeave(container.querySelector("svg")!);
    expect(onHighlightPath).toHaveBeenLastCalledWith(null);
  });

  it("labels each block with its target and duration only when asked", () => {
    const silent = render(<WorkoutProfile steps={steps} ftpWatts={FTP} powerZones={zones} />);
    expect(silent.container.querySelector(".workout-profile-block-label")).not.toBeInTheDocument();
    cleanup();

    const { container } = render(
      <WorkoutProfile steps={steps} ftpWatts={FTP} powerZones={zones} labels />,
    );
    // The 600 px fallback canvas gives the 60 s blocks about 42 px: room for "240 W" but not "Free ride".
    const labels = [...container.querySelectorAll<SVGTextElement>(".workout-profile-block-label")];
    expect(labels.map((label) => label.dataset.labelFor)).toEqual(["0", "1", "2.0", "2.0"]);
    expect(labels[2].querySelector(".workout-profile-block-target")).toHaveTextContent("240 W");
    expect(labels[2].querySelector(".workout-profile-block-duration")).toHaveTextContent("1:00");
    expect(labels[0].querySelector(".workout-profile-block-target")).toHaveTextContent("120 W");
    expect(labels[0].querySelector(".workout-profile-block-duration")).toHaveTextContent("5:00");
    expect(labels[0].dataset.placement).toBe("inside");
    expect(labels[1].querySelector(".workout-profile-block-target")).toHaveTextContent("100–220 W");
  });

  it("scales every block and label with the ride bias while FTP stays put", () => {
    const { container, rerender } = render(
      <WorkoutProfile steps={[steady(600, 100)]} ftpWatts={FTP} powerZones={zones} variant="editor" labels />,
    );
    const topOf = () => Number(polygons(container)[0].getAttribute("points")!.split(" ")[0].split(",")[1]);
    const ftpLine = () => Number(container.querySelector("[data-ftp-line]")?.getAttribute("y1"));
    expect(topOf()).toBeCloseTo(ftpLine(), 5);
    expect(container.querySelector(".workout-profile-block-target")).toHaveTextContent("200 W");

    rerender(
      <WorkoutProfile steps={[steady(600, 100)]} ftpWatts={FTP} biasPercent={110} powerZones={zones} variant="editor" labels />,
    );
    expect(container.querySelector(".workout-profile-block-target")).toHaveTextContent("220 W");
    expect(topOf()).toBeLessThan(ftpLine());
    expect(screen.getByRole("img", { name: "10:00 workout, 1 block, peak 110% FTP" })).toBeInTheDocument();
  });

  it("overlays per-block progress supplied by the host", () => {
    const { container } = render(
      <WorkoutProfile
        steps={steps}
        ftpWatts={FTP}
        powerZones={zones}
        segmentState={(_shape, index) =>
          index === 0 ? { className: "completed", progress: 1 } : index === 1 ? { className: "current", progress: 0.5 } : undefined
        }
      />,
    );
    const blocks = polygons(container);
    expect(blocks[0]).toHaveClass("completed");
    expect(blocks[1]).toHaveClass("current");
    const overlays = [...container.querySelectorAll<SVGRectElement>(".workout-profile-progress")];
    expect(overlays.map((rect) => rect.dataset.progressFor)).toEqual(["0", "1"]);
    const [[left], [right]] = blocks[1]
      .getAttribute("points")!
      .split(" ")
      .map((pair) => pair.split(",").map(Number));
    expect(Number(overlays[1].getAttribute("x"))).toBeCloseTo(left, 5);
    expect(Number(overlays[1].getAttribute("width"))).toBeCloseTo((right - left) / 2, 1);
  });
});
