// The month strip Flow and Holdings share: one button per month, latest on
// the right. The selected month is App state, so both screens read and set
// the same one. Each screen decides what a month shows (Flow: an Out bar and
// figure; Holdings: net worth at month end).
import { useEffect, useRef } from "react";

export interface StripMonth {
  /** `YYYY-MM`. */
  key: string;
  /** "Nov 2026": the accessible name's lead. */
  label: string;
  /** "Nov '26". */
  short: string;
  /** The figure under the month, already formatted. */
  figure: string;
  /** Spoken after the label, e.g. "−$1,234.00 out". */
  spoken: string;
  /** Bar height 0..1; omit for no bar. */
  bar?: number;
  /** The month containing today. */
  sofar: boolean;
  /** Needs a look (Check-in: well below its usual): an amber underline on
   *  the label; `spoken` says it in words. */
  warn?: boolean;
}

export function MonthStrip({
  months,
  selected,
  onSelect,
  caption,
}: {
  months: StripMonth[];
  selected: string | null;
  onSelect: (key: string) => void;
  /** What each month's figure is, e.g. "Out per month". */
  caption: string;
}) {
  const scroller = useRef<HTMLOListElement>(null);
  // Start scrolled to the latest months.
  useEffect(() => {
    const el = scroller.current;
    if (el) el.scrollLeft = el.scrollWidth;
  }, [months.length]);

  // One Tab stop for the whole strip (the archive only grows); arrow keys,
  // Home and End move between months and select as they go.
  const keys = months.map((m) => m.key);
  const at = selected ? keys.indexOf(selected) : -1;
  const current = at >= 0 ? at : keys.length - 1;
  const onKeyDown = (e: React.KeyboardEvent) => {
    const step: Record<string, number> = {
      ArrowLeft: current - 1,
      ArrowRight: current + 1,
      Home: 0,
      End: keys.length - 1,
    };
    if (!(e.key in step)) return;
    e.preventDefault();
    const next = Math.min(keys.length - 1, Math.max(0, step[e.key]));
    onSelect(keys[next]);
    scroller.current
      ?.querySelectorAll<HTMLButtonElement>("button")
      [next]?.focus();
  };
  const hasBars = months.some((m) => m.bar !== undefined);
  return (
    <nav className="lg-strip" aria-label="Month">
      <p className="lg-strip-key" id="lg-strip-key">
        {caption}
        <span className="lg-sr">
          . Left and right arrow keys change the month.
        </span>
      </p>
      <ol ref={scroller} aria-describedby="lg-strip-key" onKeyDown={onKeyDown}>
        {months.map((m, i) => {
          const on = m.key === selected;
          const h = Math.max(2, Math.round((m.bar ?? 0) * 48));
          return (
            <li key={m.key}>
              <button
                className={`lg-month${on ? " on" : ""}${m.warn ? " warn" : ""}`}
                aria-pressed={on}
                tabIndex={i === current ? 0 : -1}
                aria-label={`${m.label}${m.sofar ? " so far" : ""}: ${m.spoken}`}
                onClick={() => onSelect(m.key)}
              >
                {hasBars && (
                  <span className="lg-bar-track" aria-hidden="true">
                    <span className="lg-bar" style={{ height: `${h}px` }} />
                  </span>
                )}
                <span className="lg-mlabel" aria-hidden="true">
                  {m.short}
                </span>
                <span className="lg-mfig" aria-hidden="true">
                  {m.figure}
                </span>
                {m.sofar && (
                  <span className="lg-msofar" aria-hidden="true">
                    so far
                  </span>
                )}
              </button>
            </li>
          );
        })}
      </ol>
    </nav>
  );
}
