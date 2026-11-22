//! Test-only scaffolding shared by the crate's unit tests.
//!
//! `reactive_graph` is built with **`sandboxed-arenas`** whenever a workspace
//! run unifies features (`cargo test --workspace`), and in that mode creating a
//! reactive value needs a live `Arena`, not merely a current `Owner`. A bare
//! `#[test]` that builds an `RwSignal` therefore aborts with:
//!
//! ```text
//! the `sandboxed-arenas` feature is active, but no Arena is active
//!   reactive_graph-0.2.14/src/owner/arena.rs:88
//! ```
//!
//! A root `Owner::new()` allocates a fresh arena and `Owner::set()` installs it
//! (`reactive_graph-0.2.14/src/owner.rs:148-269`), which is the pattern the
//! already-passing tests in this crate spell out by hand. [`owner`] is that
//! pattern in one place, so a new test cannot forget the `set()` — and, more
//! importantly, cannot leak the owner when an assertion panics: cleanup runs
//! from `Drop` instead of from the end of the function.

use leptos::prelude::Owner;

/// A root `Owner` installed as the current one, cleaned up when dropped.
///
/// Hold it for the whole test:
///
/// ```ignore
/// #[test]
/// fn reads_a_signal() {
///     let _owner = crate::test_support::owner();
///     let value = RwSignal::new(1);
///     assert_eq!(value.get(), 1);
/// }
/// ```
///
/// Dropping it restores nothing and cleans the owner up, so one failing test
/// cannot leave its arena (or its owner) installed for the next test on the
/// same thread.
#[must_use = "the Owner must stay alive for as long as the test creates signals"]
pub(crate) struct TestOwner(Owner);

impl Drop for TestOwner {
    fn drop(&mut self) {
        self.0.cleanup();
    }
}

/// Install a fresh root `Owner` (and its `Arena`) as the current one.
pub(crate) fn owner() -> TestOwner {
    let owner = Owner::new();
    owner.set();
    TestOwner(owner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use leptos::prelude::{GetUntracked, RwSignal};

    #[test]
    fn the_guard_gives_reactive_values_a_live_arena() {
        let _owner = owner();
        let value = RwSignal::new(7_i32);
        assert_eq!(value.get_untracked(), 7);
    }

    /// The guard cleans the owner up on drop, which is what keeps one failing
    /// test from leaving an owner (and its arena) installed on the thread. The
    /// flip side is the reason the guard must live for the whole test body: a
    /// value created under it is disposed with it.
    #[test]
    fn dropping_the_guard_disposes_what_it_owned() {
        let value = {
            let _owner = owner();
            RwSignal::new(1_i32)
        };
        assert_eq!(
            value.try_get_untracked(),
            None,
            "the owner's cleanup must have disposed its values"
        );
    }
}
