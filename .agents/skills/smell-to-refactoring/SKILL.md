---
name: smell-to-refactoring
description: Reviewing a diff, or code that feels wrong without an obvious reason. Name each smell from Fowler's catalog, then apply the refactoring that removes it.
---

# Smell to refactoring

A smell is a named symptom; each has known refactorings. Name the smell before touching the code — the
name picks the fix.

1. Read the code and list each smell by name with its location.
2. For each one, pick a refactoring from the table.
3. Apply one refactoring at a time, tests green between steps.
4. Report the smells you left, each with its reason.

Done when: every smell found has a refactoring applied or a stated reason to leave it.

Yields to: surgical scope — smells outside the code under change are reported, not fixed.

| Smell | Refactorings |
|---|---|
| Mysterious Name | Change Function Declaration, Rename Variable, Rename Field |
| Duplicated Code | Extract Function, Slide Statements, Pull Up Method |
| Long Function | Extract Function, Decompose Conditional, Split Loop, Replace Temp with Query |
| Long Parameter List | Introduce Parameter Object, Preserve Whole Object, Remove Flag Argument |
| Global Data | Encapsulate Variable |
| Mutable Data | Encapsulate Variable, Split Variable, Separate Query from Modifier, Replace Derived Variable with Query |
| Divergent Change | Split Phase, Extract Class, Move Function |
| Shotgun Surgery | Move Function, Move Field, Combine Functions into Class |
| Feature Envy | Move Function, Extract Function |
| Data Clumps | Extract Class, Introduce Parameter Object |
| Primitive Obsession | Replace Primitive with Object (see `parse-dont-validate`) |
| Repeated Switches | Replace Conditional with Polymorphism |
| Loops | Replace Loop with Pipeline |
| Lazy Element | Inline Function, Inline Class |
| Speculative Generality | Remove Dead Code, Inline Function, Collapse Hierarchy |
| Temporary Field | Extract Class, Introduce Special Case |
| Message Chains | Hide Delegate, Move Function |
| Middle Man | Remove Middle Man, Inline Function |
| Insider Trading | Move Function, Move Field, Hide Delegate |
| Large Class | Extract Class, Extract Superclass |
| Alternative Classes with Different Interfaces | Change Function Declaration, Extract Superclass |
| Data Class | Encapsulate Record, Move Function |
| Refused Bequest | Replace Subclass with Delegate, Push Down Method |
| Comments | Extract Function, Change Function Declaration, Introduce Assertion |

In Rust and Go, read "class" as a struct with its methods, or a module.

Each refactoring's mechanics: `https://refactoring.com/catalog/<camelCaseName>.html`, for example
[extractFunction](https://refactoring.com/catalog/extractFunction.html).

In the wild:
- Martin Fowler, *Refactoring* (2nd ed.), chapter 3 — the source of the smells and their pairings.
- [refactoring.com/catalog](https://refactoring.com/catalog/) — every refactoring above, with mechanics.
