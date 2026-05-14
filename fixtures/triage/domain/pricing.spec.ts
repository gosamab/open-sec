import { subtotalCents, taxCents, totalCents, applyDiscount } from "./pricing";

const items = [
  { unitPriceCents: 100, quantity: 2, taxRate: 0.1 },
  { unitPriceCents: 250, quantity: 1, taxRate: 0.1 },
];

test("subtotal sums quantity * unit price", () => {
  expect(subtotalCents(items)).toBe(450);
});

test("tax is rounded per line", () => {
  expect(taxCents(items)).toBe(45);
});

test("total = subtotal + tax", () => {
  expect(totalCents(items)).toBe(495);
});

test("discount clamps to [0, 100]", () => {
  expect(() => applyDiscount(100, -1)).toThrow();
  expect(() => applyDiscount(100, 101)).toThrow();
});
