import { useState } from "react";
import type { Source, Stream } from "../../types";
import { agoLabel, cadenceLabel, dollars, trendClass, trendLabel } from "./labels";

export type SortMode = "size" | "change";

// Hand-rolled flexbox rhythm strip (not Recharts, per spec). Heights normalized
// to the series max; the final bar is the in-progress month (muted teal). The
// `big` variant (sidebar detail) has room to print each bar's dollar value
// underneath, mirroring the Outflows month chart; the compact row variant stays
// a pure sparkline.
export function RhythmStrip({ spark, big }: { spark: number[]; big?: boolean }) {
  const max = Math.max(1, ...spark.map((v) => Math.abs(v)));
  const barHeight = (v: number) => `${Math.max(6, Math.round((Math.abs(v) / max) * 100))}%`;

  if (big) {
    return (
      <div className="lg-bigrhythm">
        {spark.map((v, i) => (
          <div className="lg-barcol" key={i}>
            <div className="lg-barwrap">
              <i className={i === spark.length - 1 ? "cur" : ""} style={{ height: barHeight(v) }} />
            </div>
            <span className="lg-barval">{dollars(v)}</span>
          </div>
        ))}
      </div>
    );
  }

  return (
    <div className="lg-rhythm">
      {spark.map((v, i) => (
        <i
          key={i}
          className={i === spark.length - 1 ? "cur" : ""}
          style={{ height: barHeight(v) }}
        />
      ))}
    </div>
  );
}

export function SourceChip({ s }: { s: Source }) {
  // Full account name (e.g. "VentureOne (3419)"), colored by account kind.
  return (
    <span className={`lg-src ${s.kind.toLowerCase()}`}>
      {s.label}
      {s.pct < 100 ? ` · ${s.pct}%` : ""}
    </span>
  );
}

function rangeLabel(s: Stream): string {
  if (s.amount_min_cents === s.amount_max_cents) return `${dollars(s.median_amount_cents)} fixed`;
  return `${dollars(s.median_amount_cents)} median · ${dollars(s.amount_min_cents)}–${dollars(
    s.amount_max_cents,
  )}`;
}

export function StreamRow({
  stream,
  mode,
  active,
  onOpen,
}: {
  stream: Stream;
  mode: SortMode;
  active: boolean;
  onOpen: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const stop = (e: React.MouseEvent) => e.stopPropagation();
  const shown = expanded ? stream.sources : stream.sources.slice(0, 2);
  const extra = stream.sources.length - 2;

  return (
    <div className={`lg-row ${trendClass(stream.trend)}${active ? " active" : ""}`} onClick={onOpen}>
      <div className="lg-m">
        <div className="lg-mname">
          {stream.merchant}
          <span className="lg-badge">{cadenceLabel(stream.cadence)}</span>
        </div>
        <div className="lg-meta">
          {stream.occurrence_count} visits · last {agoLabel(stream.last_seen)}
          {shown.map((s, i) => (
            <span key={i} onClick={stop}>
              <SourceChip s={s} />
            </span>
          ))}
          {extra > 0 && !expanded && (
            <span
              className="lg-src more"
              onClick={(e) => {
                stop(e);
                setExpanded(true);
              }}
            >
              +{extra} more
            </span>
          )}
        </div>
      </div>
      <RhythmStrip spark={stream.spark_cents} />
      <div className="lg-num">
        <div className="lg-mo">
          {dollars(stream.monthly_estimate_cents)}
          <small>/mo</small>
        </div>
        {mode === "size" ? (
          <div className="lg-range">{rangeLabel(stream)}</div>
        ) : (
          <div className="lg-chg">{trendLabel(stream.trend_pct, stream.trend)}</div>
        )}
      </div>
      <div className="lg-chev">›</div>
    </div>
  );
}

export function StreamsCard({
  streams,
  mode,
  onMode,
  activeMerchant,
  onOpen,
  children,
}: {
  streams: Stream[];
  mode: SortMode;
  onMode: (m: SortMode) => void;
  activeMerchant: string | null;
  onOpen: (s: Stream) => void;
  children?: React.ReactNode;
}) {
  const sorted = [...streams].sort((a, b) =>
    mode === "change"
      ? Math.abs(b.trend_pct) - Math.abs(a.trend_pct)
      : b.monthly_estimate_cents - a.monthly_estimate_cents,
  );
  return (
    <div className="lg-card">
      <div className="lg-head">
        <h3>Recurring streams</h3>
        <div className="lg-toggle">
          <button className={mode === "size" ? "on" : ""} onClick={() => onMode("size")}>
            By size
          </button>
          <button className={mode === "change" ? "on" : ""} onClick={() => onMode("change")}>
            By change
          </button>
        </div>
      </div>
      {sorted.length === 0 ? (
        <div className="lg-empty">
          No recurring streams in this window yet.
          <br />
          Pull more history, or widen the window.
        </div>
      ) : (
        sorted.map((s) => (
          <StreamRow
            key={s.merchant}
            stream={s}
            mode={mode}
            active={activeMerchant === s.merchant}
            onOpen={() => onOpen(s)}
          />
        ))
      )}
      {children}
    </div>
  );
}
