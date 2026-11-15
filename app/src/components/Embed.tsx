// The embeddable chart (`/embed/chart?metric=…`): no shell, no nav, one
// chart sized to whatever frames it and readable from across a desk.
// agent-avatar frames it as a screen; the parameters are the strict list in
// docs/API.md "Embed views". The page checks only `theme` itself and hands
// everything else to `GET /api/chart`, which names any parameter it can't
// use, and that message is what this page shows.
//
// Display only: nothing to tap. The hidden table carries the numbers for a
// screen reader.
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "../api";
import { errMessage, formatCents, signedCents, unsignedCents } from "../format";
import type { ChartPoint, ChartView } from "../types";

const THEMES = ["auto", "light", "dark"] as const;
type Theme = (typeof THEMES)[number];

const REFRESH_MS = 15 * 60 * 1000;
const LIGHT = "(prefers-color-scheme: light)";

/** Whether the device asks for light, following it as it changes. */
function usePrefersLight(): boolean {
  const [light, setLight] = useState(() => window.matchMedia(LIGHT).matches);
  useEffect(() => {
    const m = window.matchMedia(LIGHT);
    const on = () => setLight(m.matches);
    m.addEventListener("change", on);
    return () => m.removeEventListener("change", on);
  }, []);
  return light;
}

export function Embed() {
  const params = new URLSearchParams(window.location.search);
  const themeParam = params.get("theme") ?? "auto";
  params.delete("theme");
  const query = params.toString();
  const theme: Theme | null = (THEMES as readonly string[]).includes(themeParam)
    ? (themeParam as Theme)
    : null;
  // `auto` resolves here, so the stylesheet has one light block.
  const prefersLight = usePrefersLight();
  const resolved = theme === "light" || theme === "dark" ? theme : prefersLight ? "light" : "dark";

  const [view, setView] = useState<ChartView | null>(null);
  const [error, setError] = useState<string | null>(
    theme ? null : "theme: use one of auto, light, dark",
  );

  useEffect(() => {
    if (!theme) return;
    if (window.location.pathname !== "/embed/chart") {
      setError("No such embed view. Use /embed/chart.");
      return;
    }
    // A wall screen stays up for days: refetch on a timer and whenever the
    // page comes back into view. Once a chart is up, a failed refetch keeps
    // it (the next one will try again) rather than blanking the screen.
    let live = true;
    let shown = false;
    const load = () =>
      api
        .chart(query)
        .then((v) => {
          if (!live) return;
          shown = true;
          setView(v);
          setError(null);
        })
        .catch((e) => live && !shown && setError(errMessage(e)));
    load();
    const timer = window.setInterval(load, REFRESH_MS);
    const onVisible = () => document.visibilityState === "visible" && load();
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      live = false;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [query, theme]);

  useEffect(() => {
    document.title = view ? `outflow · ${view.title}` : "outflow";
  }, [view]);

  return (
    <main className="em" data-theme={resolved}>
      {error ? (
        <div className="em-error" role="alert">
          <p className="em-kicker">Can't show this chart</p>
          <p className="em-error-msg">{error}</p>
        </div>
      ) : view ? (
        <Loaded view={view} />
      ) : (
        <p className="em-loading">Loading…</p>
      )}
    </main>
  );
}

/** The big figure: a dash when there is no data, never a made-up $0. */
function figure(view: ChartView): string {
  const c = view.headline.cents;
  if (c === null) return "\u2014";
  return view.step === "day" ? unsignedCents(c, true) : formatCents(Math.abs(c));
}

function Loaded({ view }: { view: ChartView }) {
  const { headline } = view;
  return (
    <>
      <header className="em-head">
        <h1 className="em-kicker">
          {view.title}
          <span className="em-range"> · {view.range_label}</span>
        </h1>
        <p className="em-figure">{figure(view)}</p>
        <p className="em-sub">
          {headline.label}
          {headline.change_cents !== null && (
            <>
              {" · "}
              <span className="em-change">{signedCents(headline.change_cents)}</span>
              {headline.change_label && ` ${headline.change_label}`}
            </>
          )}
        </p>
      </header>
      {view.points.length === 0 ? (
        <p className="em-empty">Nothing in this range yet.</p>
      ) : (
        <Plot view={view} />
      )}
      {view.partial_key && <p className="em-note">* {monthOf(view.partial_key)} so far</p>}
      <ChartTable view={view} />
    </>
  );
}

/** Measures the plot box so text stays at its CSS size at any frame size. */
function useSize<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setSize({ w: el.clientWidth, h: el.clientHeight });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, size] as const;
}

/**
 * Three to five round ticks (cents) covering `lo`..`hi`: the first at or
 * below `lo`, the last at or above `hi`, so the axis ends on a labelled
 * tick. The smallest 1/2/2.5/5 step that needs at most five.
 */
function ticks(lo: number, hi: number): number[] {
  if (hi === lo) {
    const d = Math.abs(hi) * 0.1 || 100_00;
    hi += d;
    lo = lo === 0 ? 0 : lo - d;
  }
  let mag = Math.max(100, 10 ** Math.floor(Math.log10((hi - lo) / 5))); // $1 at least
  for (;;) {
    for (const m of [1, 2, 2.5, 5]) {
      const step = m * mag;
      const a = Math.floor(lo / step);
      const b = Math.ceil(hi / step);
      if (b - a <= 4) return Array.from({ length: b - a + 1 }, (_, k) => Math.round((a + k) * step));
    }
    mag *= 10;
  }
}

/** A tick label exact to its step: $0, $750, $1k, $1.25k, $2.5M, −$500. */
function tickLabel(cents: number): string {
  const abs = Math.abs(cents) / 100;
  const [div, unit] = abs >= 1e6 ? [1e6, "M"] : abs >= 1e3 ? [1e3, "k"] : [1, ""];
  const v = Number((abs / div).toFixed(2));
  return `${cents < 0 ? "\u2212" : ""}$${v}${unit}`;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

function monthOf(key: string): string {
  return MONTHS[Number(key.slice(5, 7)) - 1];
}

function dayOf(key: string): string {
  return `${monthOf(key)} ${Number(key.slice(8, 10))}`;
}

function Plot({ view }: { view: ChartView }) {
  const [ref, { w, h }] = useSize<HTMLDivElement>();
  return (
    <div className="em-plot" ref={ref}>
      {w > 0 && h > 0 && (
        <svg width={w} height={h} role="img" aria-label={`${view.title}, ${view.range_label}`}>
          {view.step === "month" ? <Bars view={view} w={w} h={h} /> : <Line view={view} w={w} h={h} />}
        </svg>
      )}
    </div>
  );
}

// Plot margins in px: room for the tick labels on the left and the month
// labels below. Labels are CSS-sized (.em-tick), so these track that size.
const M = { top: 12, right: 8, bottom: 40, left: 72 };

function Grid({ ys, y, w }: { ys: number[]; y: (c: number) => number; w: number }) {
  return (
    <g>
      {ys.map((t) => (
        <g key={t}>
          <line
            className={t === 0 ? "em-base" : "em-grid"}
            x1={M.left}
            x2={w - M.right}
            y1={y(t)}
            y2={y(t)}
          />
          <text className="em-tick" x={M.left - 12} y={y(t)} dy="0.35em" textAnchor="end">
            {tickLabel(t)}
          </text>
        </g>
      ))}
    </g>
  );
}

function Bars({ view, w, h }: { view: ChartView; w: number; h: number }) {
  const pts = view.points;
  const hi = Math.max(0, ...pts.map((p) => p.cents));
  const lo = Math.min(0, ...pts.map((p) => p.cents));
  const ys = ticks(lo, hi);
  const top = ys[ys.length - 1];
  const bottom = ys[0];
  const plotH = h - M.top - M.bottom;
  const y = (c: number) => M.top + ((top - c) / (top - bottom || 1)) * plotH;
  const band = (w - M.left - M.right) / pts.length;
  const gap = Math.max(2, Math.min(band * 0.3, 24));
  const barW = Math.max(2, band - gap);
  const anyLit = pts.some((p) => p.highlighted);
  // A label under every bar up to 12, then every other (or third) one.
  const every = Math.ceil(pts.length / 12);
  const r = Math.min(4, barW / 2);

  return (
    <g>
      <Grid ys={ys} y={y} w={w} />
      {pts.map((p, i) => {
        const x = M.left + i * band + gap / 2;
        const y0 = y(0);
        const y1 = y(p.cents);
        const hgt = Math.abs(y0 - y1);
        const partial = p.key === view.partial_key;
        const cls = [
          "em-bar",
          anyLit ? (p.highlighted ? "lit" : "dim") : "",
          partial ? "partial" : "",
        ].join(" ");
        const showLabel = i % every === 0 || p.highlighted;
        return (
          <g key={p.key}>
            {hgt > 0 && (
              <path
                className={cls}
                d={roundedBar(x, Math.min(y0, y1), barW, hgt, r, p.cents >= 0)}
              />
            )}
            {showLabel && (
              <text
                className={`em-tick${p.highlighted ? " lit" : ""}`}
                x={x + barW / 2}
                y={h - M.bottom + 26}
                textAnchor="middle"
              >
                {monthOf(p.key)}
                {partial ? "*" : ""}
              </text>
            )}
          </g>
        );
      })}
    </g>
  );
}

/** A bar with its data end rounded (top for positive, bottom for negative). */
function roundedBar(x: number, y: number, bw: number, bh: number, r: number, up: boolean): string {
  const rr = Math.min(r, bh);
  if (up) {
    return `M${x},${y + bh} V${y + rr} Q${x},${y} ${x + rr},${y} H${x + bw - rr} Q${x + bw},${y} ${x + bw},${y + rr} V${y + bh} Z`;
  }
  return `M${x},${y} V${y + bh - rr} Q${x},${y + bh} ${x + rr},${y + bh} H${x + bw - rr} Q${x + bw},${y + bh} ${x + bw},${y + bh - rr} V${y} Z`;
}

function Line({ view, w, h }: { view: ChartView; w: number; h: number }) {
  const pts = view.points;
  const vals = pts.map((p) => p.cents);
  const vlo = Math.min(...vals);
  const vhi = Math.max(...vals);
  // A little air so the line never rides the frame, then out to round ticks.
  const pad = (vhi - vlo) * 0.04;
  const ys = ticks(vlo - pad, vhi + pad);
  const lo = ys[0];
  const hi = ys[ys.length - 1];
  const plotH = h - M.top - M.bottom;
  const y = (c: number) => M.top + ((hi - c) / (hi - lo)) * plotH;
  const plotW = w - M.left - M.right;
  const x = (i: number) => M.left + (pts.length === 1 ? plotW / 2 : (i / (pts.length - 1)) * plotW);
  const path = (ps: [ChartPoint, number][]) =>
    ps.map(([p, i], k) => `${k ? "L" : "M"}${x(i).toFixed(1)},${y(p.cents).toFixed(1)}`).join(" ");

  const indexed = pts.map((p, i) => [p, i] as [ChartPoint, number]);
  const lit = indexed.filter(([p]) => p.highlighted);
  const one = lit.length === 1;
  const labels = dateLabels(pts, x, one ? lit[0][1] : null);
  const last = indexed[indexed.length - 1];

  return (
    <g>
      <Grid ys={ys} y={y} w={w} />
      {lit.length > 1 && (
        <rect
          className="em-band"
          x={x(lit[0][1])}
          y={M.top}
          width={Math.max(2, x(lit[lit.length - 1][1]) - x(lit[0][1]))}
          height={plotH}
        />
      )}
      <path className={`em-line${lit.length ? " dim" : ""}`} d={path(indexed)} />
      {lit.length > 1 && <path className="em-line lit" d={path(lit)} />}
      {one && (
        <>
          <line
            className="em-rule"
            x1={x(lit[0][1])}
            x2={x(lit[0][1])}
            y1={M.top}
            y2={M.top + plotH}
          />
          <circle className="em-dot" cx={x(lit[0][1])} cy={y(lit[0][0].cents)} r={8} />
        </>
      )}
      {!lit.length && <circle className="em-dot" cx={x(last[1])} cy={y(last[0].cents)} r={5} />}
      {labels.map((l) => {
        const at = x(l.i);
        const anchor =
          at - M.left < LABEL_HALF ? "start" : w - M.right - at < LABEL_HALF ? "end" : "middle";
        return (
          <text
            key={l.i}
            className={`em-tick${l.lit ? " lit" : ""}`}
            x={at}
            y={h - M.bottom + 26}
            textAnchor={anchor}
          >
            {l.text}
          </text>
        );
      })}
    </g>
  );
}

// Half a date label's width, about: closer to the plot's edge than this,
// a label is anchored at its end instead of centred so it stays inside.
const LABEL_HALF = 36;
// Labels closer than this to the lit day's label make way for it.
const LABEL_GAP = 88;

/**
 * The line's date labels: month starts ("Oct"), at most five, counted back
 * from the latest; under two months, about five evenly spaced days. The
 * lit day, when there is one, gets its own bold label and its neighbours
 * make room.
 */
function dateLabels(
  pts: ChartPoint[],
  x: (i: number) => number,
  litAt: number | null,
): { i: number; text: string; lit: boolean }[] {
  const starts = pts.flatMap((p, i) => (p.key.endsWith("-01") ? [i] : []));
  let out: { i: number; text: string; lit: boolean }[];
  if (starts.length >= 2) {
    const every = Math.ceil(starts.length / 5);
    out = starts
      .filter((_, k) => (starts.length - 1 - k) % every === 0)
      .map((i) => ({ i, text: monthOf(pts[i].key), lit: false }));
  } else {
    const n = Math.min(5, pts.length);
    const at = new Set(
      Array.from({ length: n }, (_, k) => Math.round((k * (pts.length - 1)) / Math.max(1, n - 1))),
    );
    out = [...at].map((i) => ({ i, text: dayOf(pts[i].key), lit: false }));
  }
  if (litAt === null) return out;
  return [
    ...out.filter((l) => Math.abs(x(l.i) - x(litAt)) >= LABEL_GAP),
    { i: litAt, text: dayOf(pts[litAt].key), lit: true },
  ];
}

function ChartTable({ view }: { view: ChartView }) {
  return (
    <table className="em-sr">
      <caption>
        {view.title}, {view.range_label}
      </caption>
      <thead>
        <tr>
          <th scope="col">{view.step === "month" ? "Month" : "Day"}</th>
          <th scope="col">Amount</th>
        </tr>
      </thead>
      <tbody>
        {view.points.map((p) => (
          <tr key={p.key}>
            <th scope="row">
              {p.key}
              {p.highlighted ? " (highlighted)" : ""}
              {p.key === view.partial_key ? " (so far)" : ""}
            </th>
            <td>{formatCents(p.cents)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
