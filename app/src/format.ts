// Presentation helpers. All amounts arrive as integer cents.

export function formatCents(cents: number): string {
  const neg = cents < 0;
  const dollars = (Math.abs(cents) / 100).toLocaleString("en-US", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  });
  return `${neg ? "-" : ""}$${dollars}`;
}

/// Compact form for axis ticks and chart labels: $1.2k, $980, $3.4M.
export function formatCentsShort(cents: number): string {
  const neg = cents < 0;
  const abs = Math.abs(cents) / 100;
  let s: string;
  if (abs >= 1_000_000) s = `${(abs / 1_000_000).toFixed(1)}M`;
  else if (abs >= 1_000) s = `${(abs / 1_000).toFixed(1)}k`;
  else s = abs.toFixed(0);
  return `${neg ? "-" : ""}$${s}`;
}

const MONTHS = [
  "Jan", "Feb", "Mar", "Apr", "May", "Jun",
  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

export function monthLabel(year: number, month: number): string {
  return `${MONTHS[(month - 1 + 12) % 12]} ${year}`;
}

export function monthLabelShort(year: number, month: number): string {
  return `${MONTHS[(month - 1 + 12) % 12]} '${String(year).slice(2)}`;
}

export function formatDate(epochSecs: number): string {
  return new Date(epochSecs * 1000).toLocaleDateString("en-US", {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}
