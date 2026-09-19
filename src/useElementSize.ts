import { useLayoutEffect, useRef, useState } from "react";

export type ElementSize = { width: number; height: number };

/**
 * Tracks an element's rendered size so drawings can work in pixel space
 * instead of stretching a fixed viewBox. Until a real measurement is
 * available (first paint, or environments without layout such as jsdom) the
 * fallback size is reported, which keeps output deterministic in tests.
 */
export function useElementSize<T extends HTMLElement>(
  fallback: ElementSize,
): [React.RefObject<T | null>, ElementSize] {
  const ref = useRef<T>(null);
  const [size, setSize] = useState<ElementSize>(fallback);

  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const measure = () => {
      const rect = element.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) return;
      setSize((previous) =>
        previous.width === rect.width && previous.height === rect.height
          ? previous
          : { width: rect.width, height: rect.height },
      );
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  return [ref, size];
}
