# Rust

## Comments

Restating the code:

```rust
/// This function builds a monkey and takes a `&str`, `foo`, and a `u32`,
/// `bar`, as arguments. Returns a Result.
pub fn build_monkey(foo: &str, bar: u32) -> Result<Monkey, MonkeyError> {
    // hold the count of monkeys
    let mut count = 0;
    ...
}
```

Names carry the what; the comment carries a why the code cannot show:

```rust
pub fn build_monkey(name: &str, height_cm: u32) -> Result<Monkey, MonkeyError> {
    // Heights over 300 cm break the bed's spring model; refuse them here, not mid-fall.
    if height_cm > 300 {
        return Err(MonkeyError::TooHigh(height_cm));
    }
    ...
}
```

## Tests

Several behaviours in one test, asserting on message text:

```rust
#[test]
fn test_monkey() {
    let m = build_monkey("safe-monkey", 9).unwrap();
    assert!(trigger_fall(m).bed() == Bed::Soft, "bad bed");
    let e = build_monkey("bedless-monkey", 9).unwrap_err();
    assert_eq!(e.to_string(), "no bed configured");
}
```

One behaviour per test, named for its assertion, asserting on the error type:

```rust
#[test]
fn falls_on_soft_bed_from_low_height() {
    let monkey = Monkey::builder().height(9).name("safe-monkey").build();

    let fallen = trigger_fall(monkey);

    assert_eq!(fallen.bed(), Bed::Soft);
}

#[test]
fn refuses_to_fall_without_a_bed() {
    let monkey = Monkey::builder().height(9).build();

    let err = trigger_fall(monkey).unwrap_err();

    assert!(matches!(err, MonkeyError::NoBed));
}
```
