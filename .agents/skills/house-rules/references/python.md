# Python

## Comments

Restating the code:

```python
# This function builds a monkey and takes a string, "foo", and an int, "bar", as arguments
def build_monkey(foo: str, bar: int) -> Monkey:
    # hold the count of monkeys
    count = 0
    ...
```

Names carry the what; the comment carries a why the code cannot show:

```python
def build_monkey(name: str, height_cm: int) -> Monkey:
    # Heights over 300 cm break the bed's spring model; refuse them here, not mid-fall.
    if height_cm > 300:
        raise TooHigh(height_cm)
    ...
```

## Tests

Several behaviours in one test, asserting on message text:

```python
def test_monkey():
    m = build_monkey("safe-monkey", 9)
    assert trigger_fall(m).bed == Bed.SOFT
    try:
        trigger_fall(build_monkey("bedless-monkey", 9))
    except Exception as e:
        assert str(e) == "no bed configured"
```

One behaviour per test, named for its assertion, asserting on the exception type:

```python
def test_falls_on_soft_bed_from_low_height():
    monkey = Monkey.builder().height(9).name("safe-monkey").build()

    fallen = trigger_fall(monkey)

    assert fallen.bed == Bed.SOFT


def test_refuses_to_fall_without_a_bed():
    monkey = Monkey.builder().height(9).build()

    with pytest.raises(NoBed):
        trigger_fall(monkey)
```
