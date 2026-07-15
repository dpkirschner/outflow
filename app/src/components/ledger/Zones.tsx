import { useState } from "react";
import type { LedgerStats, LedgerView, LineItem, Stream, TransferGroup } from "../../types";
import { formatDate } from "../../format";
import { dollars } from "./labels";
import { SourceChip } from "./Streams";

export function StatStrip({ stats }: { stats: LedgerStats }) {
  return (
    <div className="lg-strip">
      <div className="lg-stat">
        <div className="k">Recurring spend</div>
        <div className="v">
          {dollars(stats.recurring_monthly_cents)}
          <small>
            {" "}
            /mo · {stats.stream_count} stream{stats.stream_count === 1 ? "" : "s"}
          </small>
        </div>
      </div>
      <div className="lg-stat">
        <div className="k">Committed</div>
        <div className="v">
          {dollars(stats.committed_monthly_cents)}
          <small> /mo · fixed</small>
        </div>
      </div>
      <div className="lg-stat">
        <div className="k">Noise</div>
        <div className="v">
          {dollars(stats.noise_total_cents)}
          <small>
            {" "}
            · {stats.noise_count} one-off{stats.noise_count === 1 ? "" : "s"}
          </small>
        </div>
      </div>
    </div>
  );
}

// A single transaction row (used in the Noise list). Clicking opens the
// promote popover keyed on the merchant (not the individual transaction).
function LineRow({ item, onOpen }: { item: LineItem; onOpen: (key: string) => void }) {
  return (
    <div className="lg-line click" onClick={() => onOpen(item.merchant_key)}>
      <span className="d">{formatDate(item.date)}</span>
      <span className="nm">{item.merchant}</span>
      <SourceChip s={item.source} />
      <span className="a">{dollars(item.amount_cents)}</span>
    </div>
  );
}

function NotableBlock({ rows }: { rows: LineItem[] }) {
  if (rows.length === 0) return null;
  return (
    <div className="lg-nblock">
      <div className="lg-nhead">Notable · one-offs over $400</div>
      {rows.map((n) => (
        <div className="lg-nrow" key={n.id}>
          <div className="nm">{n.merchant}</div>
          <div className="nd">
            {n.category ?? "uncategorized"} · {formatDate(n.date)}
            <SourceChip s={n.source} />
          </div>
          <div className="na">{dollars(n.amount_cents)}</div>
        </div>
      ))}
    </div>
  );
}

// `open`/`onToggle` optional: when supplied the fold is controlled by the
// parent (the Noise fold, so App can keep it open across a click and collapse
// it on promote); otherwise it self-manages.
function Fold({
  label,
  note,
  amount,
  children,
  open: openProp,
  onToggle,
}: {
  label: string;
  note: string;
  amount: string;
  children: React.ReactNode;
  open?: boolean;
  onToggle?: () => void;
}) {
  const [openLocal, setOpenLocal] = useState(false);
  const open = openProp ?? openLocal;
  const toggle = onToggle ?? (() => setOpenLocal((o) => !o));
  return (
    <>
      <div className="lg-foldrow" onClick={toggle}>
        <span className={`car${open ? " open" : ""}`}>▶</span>
        <span className="lbl">{label}</span>
        <span className="note">{note}</span>
        <span className="amt">{amount}</span>
      </div>
      {open && <div className="lg-foldbody">{children}</div>}
    </>
  );
}

export function LedgerZones({
  view,
  onOpen,
  onOpenNoise,
  noiseOpen,
  onNoiseToggle,
}: {
  view: LedgerView;
  onOpen: (s: Stream) => void;
  onOpenNoise: (merchantKey: string) => void;
  noiseOpen: boolean;
  onNoiseToggle: () => void;
}) {
  const { committed, transfers, notable, noise_items, stats } = view;
  const committedTotal = committed.reduce((a, s) => a + s.monthly_estimate_cents, 0);
  const transferTotal = transfers.reduce((a, t) => a + t.total_cents, 0);
  return (
    <>
      <NotableBlock rows={notable} />
      <div className="lg-fold">
        {committed.length > 0 && (
          <Fold
            label="Committed"
            note="fixed & unactionable — click a row to see its charges"
            amount={`${dollars(committedTotal)} /mo`}
          >
            {committed.map((s: Stream) => (
              <div className="lg-subr click" key={s.merchant} onClick={() => onOpen(s)}>
                <span>{s.merchant}</span>
                <span className="r">{dollars(s.monthly_estimate_cents)} /mo ›</span>
              </div>
            ))}
          </Fold>
        )}
        {transfers.length > 0 && (
          <Fold
            label="Transfers"
            note="not spending — excluded from every total"
            amount={dollars(transferTotal)}
          >
            {transfers.map((t: TransferGroup) => (
              <div className="lg-subr" key={`${t.merchant}-${t.flag}`}>
                <span>
                  {t.merchant}
                  <span style={{ color: "var(--lg-faint)" }}>
                    {" "}
                    · {t.flag === "CardPayment" ? "card payment" : "transfer"} · {t.count}×
                  </span>
                </span>
                <span className="r">{dollars(t.total_cents)}</span>
              </div>
            ))}
            <div className="lg-note">
              Money moving between your own accounts. Suppressed once, stays suppressed.
            </div>
          </Fold>
        )}
        <Fold
          label="Noise"
          note={`non-recurring — ${stats.noise_count} one-off transaction${
            stats.noise_count === 1 ? "" : "s"
          }`}
          amount={dollars(stats.noise_total_cents)}
          open={noiseOpen}
          onToggle={onNoiseToggle}
        >
          {noise_items.length === 0 ? (
            <div className="lg-note">
              Small scattered spend, nothing recurring and nothing over $400. The big one-offs are
              surfaced up in Notable.
            </div>
          ) : (
            noise_items.map((i) => <LineRow key={i.id} item={i} onOpen={onOpenNoise} />)
          )}
        </Fold>
      </div>
    </>
  );
}
