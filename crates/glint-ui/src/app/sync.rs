// SPDX-License-Identifier: MIT
//! Keeps the window's copy of the active profile the same as the engine's (PLAN §10).
//!
//! Every change in the window sends the whole profile, built on the window's copy. A copy that
//! missed a change made elsewhere would undo it at the next edit: a profile imported with
//! `glint-ctl` under the same name, or the game window the engine picked. The engine counts
//! every change of the profile in its state's revision. It answers the window's own change with
//! the revision it became, before it tells every client the new state. A newer revision than
//! the window knows is someone else's change, so the window fetches the profile again.

use std::collections::VecDeque;

#[derive(Debug, Default)]
pub struct ProfileSync {
    /// The newest revision the window's copy shows, or will once the fetch on its way arrives.
    rev: u64,
    /// How many profiles the window has sent on this connection.
    edits: u64,
    /// For each fetch on its way, how many profiles the window had sent when it asked.
    fetches: VecDeque<u64>,
}

impl ProfileSync {
    /// A new connection: the engine may be a new one, counting revisions afresh.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The window sends its profile.
    pub fn edit(&mut self) {
        self.edits += 1;
    }

    /// The window asks for the profile.
    pub fn fetch(&mut self) {
        self.fetches.push_back(self.edits);
    }

    /// The engine applied the window's profile as revision `rev`.
    pub fn applied(&mut self, rev: u64) {
        self.rev = self.rev.max(rev);
    }

    /// The engine's state names revision `rev`. True when the window must fetch the profile: a
    /// change it didn't make, with no fetch on its way. A fetch on its way shows this revision
    /// anyway: the engine answers in order, so it made the change before it read the profile.
    pub fn changed(&mut self, rev: u64) -> bool {
        if rev <= self.rev {
            return false;
        }
        self.rev = rev;
        self.fetches.is_empty()
    }

    /// A fetched profile arrived. True when the window should take it. A profile asked for
    /// before the window sent its own is older than the window's copy, and the engine applies
    /// the window's after it.
    pub fn arrived(&mut self) -> bool {
        self.fetches.pop_front().is_none_or(|edits| edits == self.edits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Connected to an engine at revision `rev`, with the first profile in.
    fn connected(rev: u64) -> ProfileSync {
        let mut s = ProfileSync::default();
        s.fetch();
        assert!(!s.changed(rev), "the fetch sent on connecting covers the first state");
        assert!(s.arrived());
        s
    }

    #[test]
    fn the_window_s_own_changes_are_not_fetched_back() {
        let mut s = connected(1);
        // The engine answers the change before it broadcasts the state.
        s.edit();
        s.applied(2);
        assert!(!s.changed(2));
        // A burst of edits, answered in order.
        s.edit();
        s.edit();
        s.applied(3);
        assert!(!s.changed(3));
        s.applied(4);
        assert!(!s.changed(4));
    }

    #[test]
    fn a_change_made_elsewhere_is_fetched_once() {
        let mut s = connected(1);
        // `glint-ctl profile import` under the same name, or the engine picking a window.
        assert!(s.changed(2));
        s.fetch();
        // Every state after it names the same revision.
        assert!(!s.changed(2));
        assert!(s.arrived());
        assert!(!s.changed(2));
        assert!(s.changed(3));
    }

    #[test]
    fn a_fetch_on_its_way_covers_newer_states() {
        let mut s = connected(1);
        assert!(s.changed(2));
        s.fetch();
        // Another change before the profile arrives: the engine reads the profile after it.
        assert!(!s.changed(3));
        assert!(s.arrived());
        assert!(!s.changed(3));
    }

    #[test]
    fn a_profile_asked_for_before_an_edit_is_older_than_the_window_s_copy() {
        let mut s = connected(1);
        assert!(s.changed(2));
        s.fetch();
        // The user changes a setting before the fetched profile arrives.
        s.edit();
        assert!(!s.arrived(), "taking it would show the profile from before the edit");
        // The engine applies the edit after the change made elsewhere: last change wins.
        s.applied(3);
        assert!(!s.changed(3));
        // Later fetches are taken again.
        assert!(s.changed(4));
        s.fetch();
        assert!(s.arrived());
    }

    #[test]
    fn a_rejected_edit_is_fetched_over() {
        let mut s = connected(1);
        s.edit();
        // The engine refused it: no revision, and the window asks for the profile.
        s.fetch();
        assert!(s.arrived());
        assert!(!s.changed(1));
    }

    #[test]
    fn a_new_connection_counts_afresh() {
        let mut s = connected(1);
        s.edit();
        s.applied(9);
        // A restarted engine starts at revision 1 again.
        s.reset();
        s.fetch();
        assert!(!s.changed(1));
        assert!(s.arrived());
        assert!(
            s.changed(2),
            "a change the new engine made must not hide behind the old count"
        );
    }
}
