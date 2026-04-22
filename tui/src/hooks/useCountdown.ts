import { useState, useEffect } from "react";
import { formatDuration } from "../format.js";

/**
 * Given `sleepUntilMs` (epoch milliseconds), return a live duration string.
 * Returns an empty string when `sleepUntilMs` is null or already elapsed.
 */
export function useCountdown(sleepUntilMs: number | null): string {
  const [display, setDisplay] = useState("");

  useEffect(() => {
    if (sleepUntilMs === null) {
      setDisplay("");
      return;
    }

    const tick = () => {
      const diffMs = Math.max(0, sleepUntilMs - Date.now());
      if (diffMs === 0) {
        setDisplay("0s");
        return;
      }
      const totalSecs = Math.ceil(diffMs / 1000);
      setDisplay(formatDuration(totalSecs * 1000));
    };

    tick();
    const id = setInterval(tick, 1000);
    return () => clearInterval(id);
  }, [sleepUntilMs]);

  return display;
}
