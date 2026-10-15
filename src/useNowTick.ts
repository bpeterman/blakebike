import { useEffect, useState } from "react";

/**
 * A clock that ticks so uptimes and "last sample" ages keep moving between
 * device events, which only arrive when a device has something to say.
 */
export function useNowTick(intervalMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(timer);
  }, [intervalMs]);
  return now;
}
