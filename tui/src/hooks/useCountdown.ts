import { useState, useEffect } from "react";

/**
 * Given `sleepUntilMs` (epoch milliseconds), return a live "MM:SS" countdown string.
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
        setDisplay("00:00");
        return;
      }
      const totalSecs = Math.ceil(diffMs / 1000);
      const m = Math.floor(totalSecs / 60);
      const s = totalSecs % 60;
      setDisplay(`${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`);
    };

    tick();
    const id = setInterval(tick, 500);
    return () => clearInterval(id);
  }, [sleepUntilMs]);

  return display;
}
