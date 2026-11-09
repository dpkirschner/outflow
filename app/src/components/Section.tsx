// A Settings section that can fold away. Sections that need nothing start
// closed and say so in one line; the title is a button (aria-expanded) and
// the section's actions stay outside it, so they work while it's closed.
import { useId, useState, type ReactNode } from "react";

export function Section({
  id,
  title,
  status,
  open: initial,
  actions,
  danger,
  children,
}: {
  /** Anchor for links into Settings (`/settings#connections`). */
  id?: string;
  title: string;
  /** One line shown beside the title, open or closed ("2 connections, all fine"). */
  status?: ReactNode;
  /** Starts open. Read once: the reader's toggle wins afterwards. */
  open: boolean;
  actions?: ReactNode;
  danger?: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(initial);
  const body = useId();
  return (
    <section id={id} className={`st-section${danger ? " st-danger" : ""}`}>
      <div className="cx-head">
        <h2>
          <button
            className="st-fold"
            aria-expanded={open}
            aria-controls={body}
            onClick={() => setOpen((o) => !o)}
          >
            <span className="st-chev" aria-hidden="true">
              <svg width="12" height="12" viewBox="0 0 16 16" focusable="false">
                <path
                  d="M6 3l5 5-5 5"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.75"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
              </svg>
            </span>
            {title}
          </button>
        </h2>
        {status && <span className="st-status">{status}</span>}
        {actions && <div className="cx-actions">{actions}</div>}
      </div>
      <div id={body} className="st-body" hidden={!open}>
        {children}
      </div>
    </section>
  );
}
