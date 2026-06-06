//! External issue **sources** that feed Harness workflow runs.
//!
//! Phase 8 (Linear) builds in slices; this is **Slice 1 — read-only discovery**:
//! a Linear GraphQL client that lists a workspace's teams / workflow states /
//! labels (to populate the future "Linear trigger block" dropdowns) and previews
//! the issues a given team+state+label filter would match. It performs **no**
//! mutations — no claiming, no status transitions, no run triggering.

pub mod linear;
