---
name: parse-dont-validate
description: Input crosses a trust boundary — CLI args, HTTP bodies, files, env vars, config. Turn it into a type that cannot hold an invalid value.
---

# Parse, don't validate

At the boundary, convert raw input into a domain type whose constructor is the only check. Everything
past the boundary takes the domain type and trusts it.

1. Find where the raw value enters.
2. Define the narrowest type that holds only valid values (`AccountId`, `NonEmpty<Vec<_>>`, `Port`).
3. Construct it once, at the boundary, returning a typed error.
4. Change downstream signatures to take the type, and delete their checks.

Done when: no function past the boundary takes the raw form, and no check repeats downstream.

Yields to: simplicity — a single call site with a trivial check stays a check until a second caller
needs the same guarantee.

In the wild:
- Alexis King, [Parse, don't validate](https://lexi-lambda.github.io/blog/2019/11/05/parse-don-t-validate/) — the source.
- Rust: `std::str::from_utf8(&[u8]) -> Result<&str, Utf8Error>` — a `&str` is proof of valid UTF-8,
  so nothing downstream checks again. `NonZeroU32::new(u32) -> Option<NonZeroU32>` does the same for zero.
- Go: `netip.ParseAddr(string) (netip.Addr, error)` — callers hold an `Addr`, never a string to re-parse.
- TypeScript: zod's `schema.parse(input)` returns the typed value or throws.
- Python: pydantic's `Model.model_validate(data)` returns a model instance or raises `ValidationError`.

Before and after: [references/rust.md](references/rust.md).
