# TypeScript

## Comments

Restating the code:

```typescript
/**
 * This function builds a monkey.
 * @param foo - a string
 * @param bar - a number
 * @returns a Monkey
 */
export function buildMonkey(foo: string, bar: number): Monkey {
  // hold the count of monkeys
  let count = 0;
  ...
}
```

Names carry the what; the comment carries a why the code cannot show:

```typescript
export function buildMonkey(name: string, heightCm: number): Monkey {
  // Heights over 300 cm break the bed's spring model; refuse them here, not mid-fall.
  if (heightCm > 300) throw new TooHighError(heightCm);
  ...
}
```

## Tests

Several behaviours in one test, asserting on message text:

```typescript
it("works", () => {
  expect(triggerFall(buildMonkey("safe-monkey", 9)).bed).toBe(Bed.Soft);
  try {
    triggerFall(buildMonkey("bedless-monkey", 9));
  } catch (e: any) {
    expect(e.message).toBe("no bed configured");
  }
});
```

One behaviour per test, named for its assertion, asserting on the error class:

```typescript
it("falls on a soft bed from a low height", () => {
  const monkey = monkeyBuilder().height(9).name("safe-monkey").build();

  const fallen = triggerFall(monkey);

  expect(fallen.bed).toBe(Bed.Soft);
});

it("refuses to fall without a bed", () => {
  const monkey = monkeyBuilder().height(9).build();

  expect(() => triggerFall(monkey)).toThrow(NoBedError);
});
```
