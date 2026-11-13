# Go

## Comments

Restating the code:

```go
// BuildMonkey builds a monkey. It takes a string, foo, and an int, bar, and
// returns a Monkey and an error.
func BuildMonkey(foo string, bar int) (*Monkey, error) {
	// hold the count of monkeys
	count := 0
	...
}
```

Names carry the what; the comment carries a why the code cannot show:

```go
func BuildMonkey(name string, heightCM int) (*Monkey, error) {
	// Heights over 300 cm break the bed's spring model; refuse them here, not mid-fall.
	if heightCM > 300 {
		return nil, ErrTooHigh
	}
	...
}
```

## Tests

Several behaviours in one test, asserting on message text:

```go
func TestBuildMonkey1(t *testing.T) {
	m, err := BuildMonkey("safe-monkey", 9)
	if err != nil && err.Error() != "bed is missing" {
		t.Fatal("it broke")
	}
	if TriggerFall(m).Bed() != BedSoft {
		t.Fatal("bad bed")
	}
}
```

One behaviour per test, named for its assertion, asserting on the error value:

```go
func TestFallsOnSoftBedFromLowHeight(t *testing.T) {
	monkey := NewMonkey().WithHeight(9).WithName("safe-monkey").Build()

	fallen := TriggerFall(monkey)

	if got := fallen.Bed(); got != BedSoft {
		t.Errorf("Bed() = %v, want %v", got, BedSoft)
	}
}

func TestRefusesToFallWithoutABed(t *testing.T) {
	monkey := NewMonkey().WithHeight(9).Build()

	_, err := TriggerFallOrErr(monkey)

	if !errors.Is(err, ErrNoBed) {
		t.Errorf("err = %v, want %v", err, ErrNoBed)
	}
}
```
