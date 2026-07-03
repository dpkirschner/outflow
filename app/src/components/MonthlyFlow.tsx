import {
  Bar,
  CartesianGrid,
  ComposedChart,
  Line,
  ReferenceLine,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import type { MonthlyFlow } from "../types";
import { formatCents, formatCentsShort, monthLabel, monthLabelShort } from "../format";

interface Props {
  flow: MonthlyFlow[];
}

// Inflow renders above the axis, outflow below (negative), net as a line —
// a diverging bar chart that reads like a waterfall of money in vs out.
export function MonthlyFlowCard({ flow }: Props) {
  const data = flow.map((f) => ({
    key: monthLabelShort(f.year, f.month),
    label: monthLabel(f.year, f.month),
    inflow: f.inflow_cents / 100,
    outflow: -f.outflow_cents / 100,
    net: f.net_cents / 100,
  }));

  return (
    <div className="card col-12">
      <h2>Monthly cash flow</h2>
      {data.length === 0 ? (
        <div className="muted">No transactions yet.</div>
      ) : (
        <>
          <ResponsiveContainer width="100%" height={280}>
            <ComposedChart data={data} margin={{ top: 8, right: 12, bottom: 0, left: 4 }}>
              <CartesianGrid stroke="#232c3d" vertical={false} />
              <XAxis
                dataKey="key"
                stroke="#5c6b82"
                tick={{ fontSize: 11 }}
                tickLine={false}
              />
              <YAxis
                stroke="#5c6b82"
                tick={{ fontSize: 11 }}
                tickLine={false}
                width={54}
                tickFormatter={(v: number) => formatCentsShort(v * 100)}
              />
              <ReferenceLine y={0} stroke="#3a4657" />
              <Tooltip
                contentStyle={{
                  background: "#1a2130",
                  border: "1px solid #232c3d",
                  borderRadius: 10,
                  fontSize: 12,
                }}
                labelStyle={{ color: "#e6edf6" }}
                formatter={(value: number, name: string) => [
                  formatCents(Math.round(value * 100)),
                  name,
                ]}
                labelFormatter={(_l, payload) =>
                  payload && payload[0] ? payload[0].payload.label : ""
                }
              />
              <Bar dataKey="inflow" name="Inflow" fill="#38d39f" radius={[3, 3, 0, 0]} />
              <Bar dataKey="outflow" name="Outflow" fill="#ff6b8a" radius={[0, 0, 3, 3]} />
              <Line
                type="monotone"
                dataKey="net"
                name="Net"
                stroke="#6ea8fe"
                strokeWidth={2}
                dot={{ r: 2 }}
              />
            </ComposedChart>
          </ResponsiveContainer>
          <div className="legend">
            <span className="item">
              <span className="dot" style={{ background: "#38d39f" }} /> Inflow
            </span>
            <span className="item">
              <span className="dot" style={{ background: "#ff6b8a" }} /> Outflow
            </span>
            <span className="item">
              <span className="dot" style={{ background: "#6ea8fe" }} /> Net
            </span>
          </div>
        </>
      )}
    </div>
  );
}
