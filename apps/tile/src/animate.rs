//! The frame pump: turns a [`tile_core::Animator`] into real window movement.
//!
//! This is the only place in Tile that sleeps in the middle of an action, so
//! it is deliberately small and free of policy. Deciding *where* a window
//! should go stays in `tile_core::Engine`; deciding *how it gets there* stays
//! in `tile_core::animation`. This module only paces the frames, pushes them at
//! the backend, and reports back when something interrupts.

use std::time::{Duration, Instant};

use tile_core::{AnimationParams, Animator, Rect, WindowAction, WindowId};
use tile_platform::{AnimationSession, WindowBackend};

/// Ceiling on the animation frame rate for this platform, if any.
///
/// On macOS every frame is a pair of synchronous Accessibility calls that
/// cross a process boundary into the app being moved, so frames are orders of
/// magnitude more expensive than a Win32 `SetWindowPos` and a high rate buys
/// nothing but contention with the app's own main thread. Elsewhere the
/// configured value already carries its own clamp, so no further cap applies.
#[cfg(target_os = "macos")]
const MAX_FPS: Option<u32> = Some(45);
#[cfg(not(target_os = "macos"))]
const MAX_FPS: Option<u32> = None;

/// Why the pump stopped.
pub enum Interruption {
    /// The animation reached its target. Carries the frame the window truly
    /// ended up with, straight from the final
    /// [`WindowBackend::set_window_frame`] — the value the engine needs for
    /// history and no-op detection.
    Settled(Rect),
    /// A new action arrived while the window was still in flight. The animator
    /// is left untouched, so the caller can retarget it and keep the momentum
    /// the window already has.
    Preempted(WindowAction),
}

/// Runs `animator` until it settles or `next` produces an action.
///
/// `session` is the backend's optional fast path for intermediate frames; it
/// is opened lazily on the first frame and left in place across a retarget, so
/// a burst of hotkeys pays for the per-animation setup once. Pass `None` in to
/// have it opened, and drop it when moving on to a different window.
///
/// Any backend error aborts immediately and propagates unchanged, so a window
/// that turns out to be unmovable (an elevated process on Windows, revoked
/// Accessibility permission on macOS) reaches the caller exactly as it would
/// from an unanimated move.
pub fn pump(
    backend: &dyn WindowBackend,
    id: WindowId,
    session: &mut Option<Box<dyn AnimationSession>>,
    animator: &mut Animator,
    params: AnimationParams,
    next: &mut dyn FnMut() -> Option<WindowAction>,
) -> tile_platform::Result<Interruption> {
    let interval = effective_interval(params);

    // The first frame has no measured history, so assume the nominal interval.
    // Subsequent frames feed the animator the time that actually elapsed,
    // which is what keeps the motion honest when a frame runs late.
    let mut dt = interval;
    let mut previous = Instant::now();

    loop {
        // Check for a newly pressed hotkey *before* spending a frame on the
        // old target, so a retarget takes effect at the next frame rather than
        // one frame late.
        if let Some(action) = next() {
            return Ok(Interruption::Preempted(action));
        }

        let frame = animator.step(dt);

        // The animator guarantees termination — it settles on both position
        // and velocity, and has a hard time budget besides — so this loop
        // needs no iteration cap of its own.
        if animator.is_settled() {
            // The final frame goes through the full backend path: it is the
            // one that has to stick, the one the app is allowed to clamp, and
            // the only one whose result anybody reads.
            let actual = backend.set_window_frame(id, animator.target())?;
            return Ok(Interruption::Settled(actual));
        }

        if session.is_none() {
            *session = backend.begin_animation(id)?;
        }
        match session.as_mut() {
            Some(open) => open.set_intermediate_frame(frame)?,
            // No fast path on this backend: fall back to the ordinary move and
            // throw away the read-back, which is meaningless mid-flight.
            None => {
                backend.set_window_frame(id, frame)?;
            }
        }

        // Sleep for whatever is left of the frame's budget. Pushing the frame
        // can itself take longer than the interval (a busy app on macOS), in
        // which case there is nothing to wait for and the next `dt` simply
        // reflects the overrun.
        let spent = previous.elapsed();
        if let Some(remaining) = interval.checked_sub(spent) {
            std::thread::sleep(remaining);
        }

        let now = Instant::now();
        dt = now.duration_since(previous);
        previous = now;
    }
}

/// The wall-clock gap between frames, after the platform cap.
fn effective_interval(params: AnimationParams) -> Duration {
    let fps = match MAX_FPS {
        Some(cap) => params.fps.min(cap),
        None => params.fps,
    };
    AnimationParams { fps, ..params }.frame_interval()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_platform_cap_bounds_the_frame_rate() {
        // The highest rate `Config::normalize` will ever hand over.
        let params = AnimationParams {
            duration_ms: 140,
            fps: 240,
        };
        let expected = AnimationParams {
            fps: MAX_FPS.unwrap_or(params.fps),
            ..params
        };
        assert_eq!(effective_interval(params), expected.frame_interval());
    }

    #[test]
    fn a_rate_below_the_cap_is_left_alone() {
        let params = AnimationParams {
            duration_ms: 140,
            fps: 15,
        };
        assert_eq!(effective_interval(params), params.frame_interval());
    }
}
