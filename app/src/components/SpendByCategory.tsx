import { Cell, Pie, PieChart, ResponsiveContainer, Tooltip } from "recharts";
import type { CategorySpend } from "../types";
import { formatCents } from "../format";

interface Props {
  rows: CategorySpend[];
}

const PALETTE = [
  "#2de2c5", "#6ea8fe", "#ff6b8a", "#ffcf5c", "#a78bfa",
  "#38d39f", "#f59e6b", "#4dd4e8", "#e879f9", "#7dd3fc",
  "#facc15", "#fb7185",
];

export function SpendByCategory({ rows }: Props) {
  const data = rows
    .map((r) => ({
      name: r.category ?? "Uncategorized",
      value: r.total_cents,
      count: r.count,
    }))
    .filter((d) => d.value > 0);

  const total = data.reduce((s, d) => s + d.value, 0);

  return (
    <div className="card col-5">
      <h2>Spend by category</h2>
      {data.length === 0 ? (
        <div className="muted">Nothing to show.</div>
      ) : (
        <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
          <ResponsiveContainer width={190} height={220}>
            <PieChart>
              <Pie
                data={data}
                dataKey="value"
                nameKey="name"
                innerRadius={58}
                outerRadius={92}
                paddingAngle={1.5}
                stroke="none"
              >
                {data.map((d, i) => (
                  <Cell
                    key={d.name}
                    fill={d.name === "Uncategorized" ? "#3a4657" : PALETTE[i % PALETTE.length]}
                  />
                ))}
              </Pie>
              <Tooltip
                contentStyle={{
                  background: "#1a2130",
                  border: "1px solid #232c3d",
                  borderRadius: 10,
                  fontSize: 12,
                }}
                formatter={(value: number, name: string) => [formatCents(value), name]}
              />
            </PieChart>
          </ResponsiveContainer>
          <div className="scroll-y" style={{ flex: 1, minWidth: 140, maxHeight: 220 }}>
            {data.map((d, i) => (
              <div className="bar-row" key={d.name} style={{ padding: "4px 0" }}>
                <div style={{ display: "flex", alignItems: "center", gap: 8, minWidth: 0 }}>
                  <span
                    className="dot"
                    style={{
                      width: 10,
                      height: 10,
                      borderRadius: 3,
                      background:
                        d.name === "Uncategorized" ? "#3a4657" : PALETTE[i % PALETTE.length],
                      flex: "0 0 auto",
                    }}
                  />
                  <span className="bar-label">
                    <span className="m">{d.name}</span>
                  </span>
                </div>
                <span className="bar-amt">
                  {formatCents(d.value)}
                  <span style={{ color: "var(--text-faint)", fontWeight: 400 }}>
                    {" "}
                    · {total > 0 ? Math.round((d.value / total) * 100) : 0}%
                  </span>
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
