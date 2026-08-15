# 0122 Import items from their defining module, not via crate-root aliases

Level: SHOULD
Scope: Rust code in this repository; applies to new code and to code being restructured.

A `use` declaration is itself an item, and Rust's ancestor-privacy rule makes even private
items of an ancestor module — including its private `use` imports — reachable from child
modules via `crate::X`. A private import in `main.rs` therefore silently becomes a crate-wide
alias surface: `crate::task_field` may resolve through `main.rs`'s import of
`crate::task::task_field` without anyone declaring that dependency.

- Reference an item by its defining module's path (`use crate::task::task_field;`), never by a
  `crate::X` path that only resolves through another module's import list.
- When moving code between modules, treat any `crate::X` reference that stops compiling after an
  import-list cleanup as this defect: repoint it to the defining module.

Rationale (measured 2026-07-19, MAIN-SPLIT): while decomposing main.rs, modules kept compiling
through main.rs's leftover import lists, hiding true dependency edges; deleting "unused" imports
from main.rs then broke sibling modules at a distance. The split only became order-independent
after all cross-module references were repointed to their defining modules.

Enforcement: convention; review. (A lint pass — e.g. forbidding `use crate::{...}` of
non-root-defined items in new modules — may be added to the gate later.)
