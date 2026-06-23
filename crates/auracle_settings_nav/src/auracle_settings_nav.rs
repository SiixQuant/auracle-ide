//! Whether a Settings navbar item shows in the current scope, as a pure
//! function over plain data.
//!
//! The native Settings window can be viewed against several scopes (the user
//! profile, a project/worktree, a remote server). Each item carries a bitmask
//! of the scopes it belongs to, and the window normally hides an item whenever
//! the current scope is not in that mask. That masking is correct for native
//! settings — a project-only override has no meaning in the user scope — but it
//! is wrong for the handful of Auracle surfaces that are user-global by nature
//! (account / license, model providers, data sources). Those must stay
//! reachable no matter which scope happens to be selected, so the window can
//! land on a project file the moment a worktree is open.
//!
//! Keeping the decision here — a `u32`-in, `bool`-out function with no GPUI
//! dependency — means it can be exhaustively unit-tested without the platform
//! graphics toolchain, and the window's filtering code becomes a single call.

/// Returns whether an item should be visible in the current scope.
///
/// * `item_scopes_mask` — the bitmask of scopes the item belongs to (e.g. the
///   user bit, the project bit). This is the item's existing `files` mask.
/// * `current_scope_mask` — the bitmask of the scope currently being viewed.
///   This is a single scope bit.
/// * `always_global` — when `true`, the item is a user-global Auracle surface
///   that must remain visible in every scope regardless of the masks.
///
/// The item is visible when it is flagged `always_global`, or when its scope
/// mask intersects the current scope (the same rule native items already use).
pub fn item_visible(item_scopes_mask: u32, current_scope_mask: u32, always_global: bool) -> bool {
    always_global || (item_scopes_mask & current_scope_mask) != 0
}

#[cfg(test)]
mod tests {
    use super::item_visible;

    // Mirror the scope bits used by the settings window so the tests read like
    // the call site. The exact values do not matter to the reducer; only that
    // they are distinct single bits.
    const USER: u32 = 1 << 0;
    const PROJECT: u32 = 1 << 2;
    const SERVER: u32 = 1 << 3;

    #[test]
    fn user_item_visible_in_user_scope() {
        assert!(item_visible(USER, USER, false));
    }

    #[test]
    fn user_item_hidden_in_project_scope() {
        assert!(!item_visible(USER, PROJECT, false));
    }

    #[test]
    fn user_item_hidden_in_server_scope() {
        assert!(!item_visible(USER, SERVER, false));
    }

    #[test]
    fn always_global_user_item_visible_in_project_scope() {
        // The whole point of #250: a user-tagged surface flagged always-global
        // stays reachable even when a project file is the current scope.
        assert!(item_visible(USER, PROJECT, true));
    }

    #[test]
    fn always_global_visible_in_every_scope() {
        for scope in [USER, PROJECT, SERVER] {
            assert!(item_visible(USER, scope, true));
        }
    }

    #[test]
    fn always_global_visible_even_with_empty_scope_mask() {
        // always_global wins regardless of the item's own mask.
        assert!(item_visible(0, PROJECT, true));
    }

    #[test]
    fn project_item_visible_in_project_scope() {
        assert!(item_visible(PROJECT, PROJECT, false));
    }

    #[test]
    fn multi_scope_item_visible_when_any_bit_intersects() {
        let user_and_project = USER | PROJECT;
        assert!(item_visible(user_and_project, USER, false));
        assert!(item_visible(user_and_project, PROJECT, false));
        assert!(!item_visible(user_and_project, SERVER, false));
    }

    #[test]
    fn no_intersection_is_hidden_without_flag() {
        assert!(!item_visible(USER, PROJECT, false));
        assert!(!item_visible(PROJECT, USER, false));
    }

    #[test]
    fn empty_item_mask_is_hidden_without_flag() {
        assert!(!item_visible(0, USER, false));
    }
}
