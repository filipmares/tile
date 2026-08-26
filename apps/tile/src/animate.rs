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

/// Ceiling on the animation frame rate imposed by a platform, if any.
///
/// On macOS every frame is a pair of synchronous Accessibility calls that
/// cross a process boundary into the app being moved, so frames are orders of
/// magnitude more expensive than a Win32 `SetWindowPos` and a high rate buys
/// nothing but contention with the app's own main thread. Elsewhere the
/// configured value already carries its own clamp, so no further cap applies.
///
/// # Why every cap is named here rather than `#[cfg]`-selected
///
/// These are plain numbers, not platform APIs, so there is no reason a test on
/// one host cannot exercise another host's value — and every reason it should.
/// A `#[cfg]` on the constant alone makes the macOS arithmetic unreachable from
/// a Windows machine, which is how a frame-rate-dependent assertion reached CI
/// and failed there twice. Only [`PLATFORM_FPS_CAP`] is selected by platform;
/// the capping logic takes the cap as an argument so all of it is covered
/// everywhere. This mirrors the reason `tile-platform` compiles its
/// `macos_pure` helpers on non-macOS hosts.
/// [`allow(dead_code)`]: on any given host one of these caps is only ever used
/// by tests, since [`PLATFORM_FPS_CAP`] selects the other. That is the point —
/// the same reason `tile-platform` marks its `macos_pure` module the same way.
#[allow(dead_code)]
pub(crate) const MACOS_FPS_CAP: Option<u32> = Some(45);

/// The cap on platforms whose per-frame cost is a cheap system call.
#[allow(dead_code)]
pub(crate) const UNCAPPED: Option<u32> = None;

/// Every cap Tile ships, so a rate-sensitive test can cover all of them from
/// whichever host it happens to run on.
#[allow(dead_code)]
pub(crate) const ALL_FPS_CAPS: &[Option<u32>] = &[UNCAPPED, MACOS_FPS_CAP];

/// The cap that applies to the platform this build targets.
#[cfg(target_os = "macos")]
pub(crate) const PLATFORM_FPS_CAP: Option<u32> = MACOS_FPS_CAP;
#[cfg(not(target_os = "macos"))]
pub(crate) const PLATFORM_FPS_CAP: Option<u32> = UNCAPPED;

/// Why the pump stopped.
pub enum Interruption {
    /// The animation reached its target. Carries the frame the window truly
    /// ended up with — from [`AnimationSession::finish`] when the backend
    /// offered a session, or from [`WindowBackend::set_window_frame`] when it
    /// did not. Either way it is the read-back the engine needs for history
    /// and no-op detection, not the frame that was requested.
    Settled(Rect),
    /// A new action arrived while the window was still in flight. The animator
    /// is left untouched, so the caller can retarget it and keep the momentum
    /// the window already has.
    Preempted(WindowAction),
}

/// Paces the animation, one call per emitted frame.
///
/// This exists so the pump's timing is injectable. Frame *count* is a function
/// of how long each frame actually took, which on a loaded machine is anyone's
/// guess — correct behaviour for a real animation, since a late frame should
/// advance the motion further rather than play in slow motion, but impossible
/// to assert against. Tests supply a pacer that reports the nominal interval
/// without sleeping, which makes them both deterministic and instant.
pub trait Pacer {
    /// Starts the clock for a run of frames.
    ///
    /// Called immediately before the first frame of each pump, so the time
    /// spent planning the action and opening the backend session — which on
    /// macOS can run to its setup timeout — is never charged to a frame.
    /// Without this the second frame would receive the whole setup duration as
    /// its `dt` and the animation would visibly jump.
    fn reset(&mut self);

    /// Waits out the remainder of this frame's `interval` and returns how much
    /// wall-clock time the frame consumed in total. That becomes the next
    /// `dt` handed to the animator.
    fn wait(&mut self, interval: Duration) -> Duration;

    /// Starts or resumes diagnostics for one retained window flight.
    fn begin_stats(&mut self, _id: WindowId, _interval: Duration) {}

    /// Records the work performed before frame pacing sleeps.
    fn record_frame_work(&mut self, _elapsed: Duration) {}

    /// Emits and clears the aggregate diagnostic for a completed flight.
    fn finish_stats(&mut self, _outcome: &'static str) {}
}

/// The real pacer: sleeps out whatever is left of the frame's budget.
pub struct SleepPacer {
    previous: Instant,
    stats: Option<PumpStats>,
}

struct PumpStats {
    id: WindowId,
    started: Instant,
    interval: Duration,
    frames: u32,
    overruns: u32,
}

impl PumpStats {
    fn new(id: WindowId, interval: Duration) -> Self {
        Self {
            id,
            started: Instant::now(),
            interval,
            frames: 0,
            overruns: 0,
        }
    }

    fn record_frame(&mut self, elapsed: Duration) {
        self.frames += 1;
        if elapsed > self.interval {
            self.overruns += 1;
        }
    }

    fn log(&self, outcome: &'static str) {
        log::debug!(
            "animation pump summary: outcome={} duration_ms={} interval_us={} frames={} overruns={}",
            outcome,
            self.started.elapsed().as_millis(),
            self.interval.as_micros(),
            self.frames,
            self.overruns
        );
    }
}

impl SleepPacer {
    pub fn new() -> Self {
        Self {
            previous: Instant::now(),
            stats: None,
        }
    }
}

impl Pacer for SleepPacer {
    fn reset(&mut self) {
        self.previous = Instant::now();
    }

    fn wait(&mut self, interval: Duration) -> Duration {
        // Pushing the frame can itself take longer than the interval (a busy
        // app on macOS), in which case there is nothing to wait for and the
        // returned duration simply reflects the overrun.
        let spent = self.previous.elapsed();
        if let Some(remaining) = interval.checked_sub(spent) {
            std::thread::sleep(remaining);
        }
        let now = Instant::now();
        let elapsed = now.duration_since(self.previous);
        self.previous = now;
        elapsed
    }

    fn begin_stats(&mut self, id: WindowId, interval: Duration) {
        if self.stats.as_ref().is_some_and(|stats| stats.id != id) {
            if let Some(stats) = self.stats.take() {
                stats.log("superseded");
            }
        }
        if self.stats.is_none() {
            self.stats = Some(PumpStats::new(id, interval));
        }
    }

    fn record_frame_work(&mut self, elapsed: Duration) {
        if let Some(stats) = &mut self.stats {
            stats.record_frame(elapsed);
        }
    }

    fn finish_stats(&mut self, outcome: &'static str) {
        if let Some(stats) = self.stats.take() {
            stats.log(outcome);
        }
    }
}

impl Drop for SleepPacer {
    fn drop(&mut self) {
        self.finish_stats("aborted");
    }
}

/// Runs `animator` until it settles or `next` produces an action.
///
/// `session` is the backend's fast path for intermediate frames. It is opened
/// once when the flight starts, not here, and is kept across retargets of the
/// same window so a burst of hotkeys pays for the per-animation setup once. A
/// `None` means the backend declined the fast path, and this deliberately does
/// not retry — every frame simply falls back to
/// [`WindowBackend::set_window_frame`].
///
/// Any backend error aborts immediately and propagates unchanged, so a window
/// that turns out to be unmovable (an elevated process on Windows, revoked
/// Accessibility permission on macOS) reaches the caller exactly as it would
/// from an unanimated move. The caller is responsible for reconciling a window
/// left part-way through.
pub fn pump(
    backend: &dyn WindowBackend,
    id: WindowId,
    session: &mut Option<Box<dyn AnimationSession>>,
    animator: &mut Animator,
    params: AnimationParams,
    pacer: &mut dyn Pacer,
    next: &mut dyn FnMut() -> Option<WindowAction>,
) -> tile_platform::Result<Interruption> {
    let interval = effective_interval(params);

    // Start the clock now, not when the pacer was created: planning and
    // backend setup happened in between and must not be billed to a frame.
    pacer.reset();
    pacer.begin_stats(id, interval);

    // The first frame has no measured history, so assume the nominal interval.
    // Subsequent frames feed the animator the time that actually elapsed,
    // which is what keeps the motion honest when a frame runs late.
    let mut dt = interval;
    // Whether this pump has put anything on screen yet. A held shortcut
    // auto-repeats — the Windows hook forwards repeats deliberately — so at a
    // low frame rate an action can be waiting at every check. Preempting
    // before ever stepping would let the animation be starved indefinitely
    // while the channel grows, so the first frame is always drawn.
    let mut drawn = false;

    loop {
        let frame_started = Instant::now();
        // Check for a newly pressed hotkey *before* spending a frame on the
        // old target, so a retarget takes effect at the next frame rather than
        // one frame late.
        if drawn {
            if let Some(action) = next() {
                return Ok(Interruption::Preempted(action));
            }
        }

        let frame = animator.step(dt);

        // The animator guarantees termination — it settles on both position
        // and velocity, and has a hard time budget besides — so this loop
        // needs no iteration cap of its own.
        if animator.is_settled() {
            // The final frame is the one that has to stick, the one the app is
            // allowed to clamp, and the only one whose result anybody reads.
            // Prefer the session: it lands the window through the handle we
            // have been driving all along, so a focus change mid-animation
            // cannot make this fail and strand the window.
            let target = animator.target();
            let result = match session.as_mut() {
                Some(open) => open.finish(target),
                None => backend.set_window_frame(id, target),
            };
            let actual = match result {
                Ok(actual) => actual,
                Err(err) => {
                    pacer.finish_stats("error");
                    return Err(err);
                }
            };
            pacer.finish_stats("settled");
            return Ok(Interruption::Settled(actual));
        }

        let result = match session.as_mut() {
            Some(open) => open.set_intermediate_frame(frame),
            // No fast path on this backend: fall back to the ordinary move and
            // throw away the read-back, which is meaningless mid-flight. The
            // session is opened once when the flight starts, so there is
            // deliberately no lazy open here — retrying it every frame would
            // repeat the backend's setup for any backend that declines.
            None => backend.set_window_frame(id, frame).map(|_| ()),
        };
        if let Err(err) = result {
            pacer.finish_stats("error");
            return Err(err);
        }
        drawn = true;
        pacer.record_frame_work(frame_started.elapsed());

        // Hand pacing to the injected pacer, which reports how long the frame
        // really took so the animator advances by that much rather than by the
        // interval we hoped for.
        dt = pacer.wait(interval);
    }
}

/// The wall-clock gap between frames on this platform.
pub(crate) fn effective_interval(params: AnimationParams) -> Duration {
    interval_with_cap(params, PLATFORM_FPS_CAP)
}

/// The wall-clock gap between frames under an arbitrary cap.
///
/// Takes the cap rather than reading the platform constant so the macOS
/// arithmetic is exercised by tests running on any host.
pub(crate) fn interval_with_cap(params: AnimationParams, cap: Option<u32>) -> Duration {
    let fps = match cap {
        Some(cap) => params.fps.min(cap),
        None => params.fps,
    };
    AnimationParams { fps, ..params }.frame_interval()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tile_platform::{PermissionStatus, PlatformError};

    struct ErrorBackend;

    impl WindowBackend for ErrorBackend {
        fn focused_window(&self) -> tile_platform::Result<Option<tile_core::WindowSnapshot>> {
            unreachable!("pump does not query the focused window")
        }

        fn screens(&self) -> tile_platform::Result<Vec<tile_core::Screen>> {
            unreachable!("pump does not enumerate screens")
        }

        fn set_window_frame(&self, _id: WindowId, _target: Rect) -> tile_platform::Result<Rect> {
            Err(PlatformError::os("test frame", "injected failure"))
        }

        fn permission_status(&self, _prompt: bool) -> tile_platform::Result<PermissionStatus> {
            Ok(PermissionStatus::NotRequired)
        }
    }

    #[derive(Default)]
    struct RecordingPacer {
        outcomes: Vec<&'static str>,
    }

    impl Pacer for RecordingPacer {
        fn reset(&mut self) {}

        fn wait(&mut self, interval: Duration) -> Duration {
            interval
        }

        fn finish_stats(&mut self, outcome: &'static str) {
            self.outcomes.push(outcome);
        }
    }

    #[test]
    fn sleep_pacer_aggregates_stats_across_same_window_retargets() {
        let mut pacer = SleepPacer::new();
        pacer.begin_stats(7, Duration::from_millis(20));
        pacer.record_frame_work(Duration::from_millis(10));
        pacer.begin_stats(7, Duration::from_millis(20));
        pacer.record_frame_work(Duration::from_millis(21));

        let stats = pacer.stats.as_ref().unwrap();
        assert_eq!(stats.frames, 2);
        assert_eq!(stats.overruns, 1);
        pacer.finish_stats("settled");
        assert!(pacer.stats.is_none());
    }

    #[test]
    fn sleep_pacer_starts_fresh_stats_for_a_different_window() {
        let mut pacer = SleepPacer::new();
        pacer.begin_stats(7, Duration::from_millis(20));
        pacer.record_frame_work(Duration::from_millis(21));

        pacer.begin_stats(8, Duration::from_millis(20));

        let stats = pacer.stats.as_ref().unwrap();
        assert_eq!(stats.id, 8);
        assert_eq!(stats.frames, 0);
    }

    #[test]
    fn intermediate_frame_errors_finish_pump_stats() {
        let params = AnimationParams {
            duration_ms: 250,
            fps: 60,
        };
        let mut animator = Animator::new(
            Rect::new(0.0, 0.0, 100.0, 100.0),
            Rect::new(100.0, 0.0, 100.0, 100.0),
            params,
        );
        let mut pacer = RecordingPacer::default();

        let result = pump(
            &ErrorBackend,
            1,
            &mut None,
            &mut animator,
            params,
            &mut pacer,
            &mut || None,
        );

        assert!(result.is_err());
        assert_eq!(pacer.outcomes, ["error"]);
    }

    #[test]
    fn final_frame_errors_finish_pump_stats() {
        let params = AnimationParams {
            duration_ms: 250,
            fps: 60,
        };
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let mut animator = Animator::new(rect, rect, params);
        let mut pacer = RecordingPacer::default();

        let result = pump(
            &ErrorBackend,
            1,
            &mut None,
            &mut animator,
            params,
            &mut pacer,
            &mut || None,
        );

        assert!(result.is_err());
        assert_eq!(pacer.outcomes, ["error"]);
    }

    #[test]
    fn every_platform_cap_bounds_the_frame_rate() {
        // Both branches run on every host, so the macOS capping arithmetic is
        // covered from a Windows or Linux machine too.
        let params = AnimationParams {
            duration_ms: 340,
            fps: 240,
        };

        assert_eq!(
            interval_with_cap(params, MACOS_FPS_CAP),
            AnimationParams { fps: 45, ..params }.frame_interval()
        );
        assert_eq!(
            interval_with_cap(params, UNCAPPED),
            params.frame_interval(),
            "an uncapped platform should use the configured rate verbatim"
        );
    }

    #[test]
    fn a_rate_below_every_cap_is_left_alone() {
        let params = AnimationParams {
            duration_ms: 340,
            fps: 15,
        };
        for cap in ALL_FPS_CAPS {
            assert_eq!(
                interval_with_cap(params, *cap),
                params.frame_interval(),
                "cap {cap:?} changed a rate already below it"
            );
        }
    }

    #[test]
    fn this_platform_uses_one_of_the_declared_caps() {
        assert!(ALL_FPS_CAPS.contains(&PLATFORM_FPS_CAP));
        assert_eq!(
            effective_interval(AnimationParams {
                duration_ms: 340,
                fps: 240,
            }),
            interval_with_cap(
                AnimationParams {
                    duration_ms: 340,
                    fps: 240,
                },
                PLATFORM_FPS_CAP
            )
        );
    }
}
