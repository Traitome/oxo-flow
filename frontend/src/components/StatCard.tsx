/** Shared stat-card markup (`.stat-card` / `.stat-value` / `.stat-label`). */
export default function StatCard({
  value,
  label,
  valueStyle,
}: {
  value: React.ReactNode;
  label: string;
  valueStyle?: React.CSSProperties;
}) {
  return (
    <div className="stat-card">
      <div className="stat-value" style={valueStyle}>{value}</div>
      <div className="stat-label">{label}</div>
    </div>
  );
}
