/**
 * Pairs each row of a fixed server list with its position. The lists the screens show (report
 * components and evidence, review rows) come from Rust in one order and are never reordered or
 * edited, so the position is a stable key; a row has no identifier of its own.
 */
export function keyedRows<T>(rows: readonly T[]): { key: string; row: T }[] {
  return rows.map((row, index) => ({ key: String(index), row }));
}
