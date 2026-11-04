const SLOW_FRAME_MS = 50;

export interface FrameStats {
  frames: number;
  p50: number;
  p95: number;
  p99: number;
  over50ms: number;
}

export function frameStats(deltasMs: number[]): FrameStats {
  const sorted = [...deltasMs].sort((a, b) => a - b);
  const percentile = (rank: number) =>
    sorted.length === 0 ? 0 : sorted[Math.ceil((rank / 100) * sorted.length) - 1];
  return {
    frames: sorted.length,
    p50: percentile(50),
    p95: percentile(95),
    p99: percentile(99),
    over50ms: sorted.filter((delta) => delta > SLOW_FRAME_MS).length,
  };
}
