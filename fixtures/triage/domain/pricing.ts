// Pure-function pricing logic. No I/O, no external input handling.

export interface LineItem {
  unitPriceCents: number;
  quantity: number;
  taxRate: number;
}

export function subtotalCents(items: LineItem[]): number {
  return items.reduce((s, i) => s + i.unitPriceCents * i.quantity, 0);
}

export function taxCents(items: LineItem[]): number {
  return items.reduce(
    (s, i) => s + Math.round(i.unitPriceCents * i.quantity * i.taxRate),
    0,
  );
}

export function totalCents(items: LineItem[]): number {
  return subtotalCents(items) + taxCents(items);
}

export function applyDiscount(totalCents: number, percent: number): number {
  if (percent < 0 || percent > 100) {
    throw new Error("percent must be between 0 and 100");
  }
  return Math.round(totalCents * (1 - percent / 100));
}
