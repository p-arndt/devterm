//! Frame-timing policy: the pure decision of *whether* to present a frame.
//!
//! The event loop drives presents from two sources: high-frequency PTY output (which we
//! coalesce so a byte burst paints once) and low-frequency user/UI changes (which paint
//! immediately). [`should_present`] folds both into a single, side-effect-free predicate
//! so it can be unit-tested without a display; the timing constants it reads are shared
//! with the scheduler in `about_to_wait`.

use std::time::Duration;

/// Quiet gap after the most recent PTY byte before a burst is considered "settled" and
/// worth painting. Short enough to feel instant, long enough to fold a flurry of reads
/// into one frame.
pub(super) const SETTLE_WINDOW: Duration = Duration::from_micros(1_500);

/// Upper bound on how long a *continuously* noisy pane may defer a present. Once this much
/// time has passed since the last frame we paint regardless of the settle window, giving a
/// steady ~60 Hz cadence instead of starving the display.
pub(super) const MAX_DEFER: Duration = Duration::from_millis(16);

/// Cursor blink half-period: the focused cursor toggles visibility on this interval.
pub(super) const BLINK_INTERVAL: Duration = Duration::from_millis(500);

/// Decide whether the current frame should be presented.
///
/// - `any_dirty` — at least one pane's terminal changed since it was last snapshotted.
/// - `force` — a non-terminal change (focus move, split/close, resize, selection, blink
///   toggle, theme/font change) requires a repaint even when no terminal is dirty.
/// - `in_sync` — the focused pane is mid DECSET-2026 synchronized update; painting now
///   would tear, so we defer until it ends (the end sequence arrives as more output).
/// - `since_last_byte` — elapsed time since the most recent PTY output.
/// - `since_last_present` — elapsed time since the last presented frame.
///
/// Synchronized updates suppress every present (matching the tear-free contract). Otherwise
/// a forced change paints immediately; a dirty terminal paints once its byte burst has
/// settled ([`SETTLE_WINDOW`]) or once the deferral cap ([`MAX_DEFER`]) is hit; an
/// unchanged, unforced frame is skipped entirely.
pub(super) fn should_present(
    any_dirty: bool,
    force: bool,
    in_sync: bool,
    since_last_byte: Duration,
    since_last_present: Duration,
) -> bool {
    if in_sync {
        return false;
    }
    if force {
        return true;
    }
    if !any_dirty {
        return false;
    }
    since_last_byte >= SETTLE_WINDOW || since_last_present >= MAX_DEFER
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZERO: Duration = Duration::ZERO;
    const LONG: Duration = Duration::from_secs(1);

    #[test]
    fn sync_suppresses_every_present() {
        // Even a forced, dirty, long-settled frame is held back mid synchronized update.
        assert!(!should_present(true, true, true, LONG, LONG));
        assert!(!should_present(false, false, true, LONG, LONG));
    }

    #[test]
    fn force_paints_immediately_when_not_in_sync() {
        // No dirty terminal and a fresh byte, but a forced change still paints.
        assert!(should_present(false, true, false, ZERO, ZERO));
    }

    #[test]
    fn nothing_to_do_is_skipped() {
        assert!(!should_present(false, false, false, LONG, LONG));
    }

    #[test]
    fn dirty_waits_for_the_settle_window() {
        // Fresh byte, well under the deferral cap: keep coalescing.
        assert!(!should_present(true, false, false, ZERO, ZERO));
        // A byte just under the window still waits.
        let almost = SETTLE_WINDOW - Duration::from_micros(1);
        assert!(!should_present(true, false, false, almost, ZERO));
    }

    #[test]
    fn dirty_paints_once_settled() {
        assert!(should_present(true, false, false, SETTLE_WINDOW, ZERO));
        assert!(should_present(true, false, false, LONG, ZERO));
    }

    #[test]
    fn dirty_paints_when_deferral_cap_is_hit() {
        // Still receiving bytes (not settled), but we have starved the display long
        // enough: paint to keep a steady cadence.
        assert!(should_present(true, false, false, ZERO, MAX_DEFER));
        assert!(should_present(true, false, false, ZERO, LONG));
    }
}
