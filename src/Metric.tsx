export function Metric({ value, unit }: { value: string; unit: string }) {
  return <div className="metric"><strong>{value}</strong><span>{unit}</span></div>;
}
