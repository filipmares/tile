//! Pure, finite-duration window animation.
//!
//! The animator emits a rigid rectangle along one monotonic ease-out path. It
//! has no clock or I/O: callers provide elapsed time and apply the returned
//! frame, which keeps the motion deterministic and unit-testable on every host.

use std::time::Duration;

use crate::geometry::Rect;

/// The compile-time-selected tuning profile for the current desktop platform.
#[derive(Debug, Clone, Copy, PartialEq)]
struct AnimationProfile {
    default_duration_ms: u32,
    initial_slope: f64,
}

#[allow(dead_code)]
const MACOS_PROFILE: AnimationProfile = AnimationProfile {
    default_duration_ms: 250,
    // A slightly softer ease-out suits the higher-cost Accessibility path.
    initial_slope: 2.6,
};

#[allow(dead_code)]
const WINDOWS_PROFILE: AnimationProfile = AnimationProfile {
    default_duration_ms: 220,
    initial_slope: 3.0,
};

#[allow(dead_code)]
const OTHER_PROFILE: AnimationProfile = WINDOWS_PROFILE;

#[cfg(target_os = "macos")]
const PLATFORM_PROFILE: AnimationProfile = MACOS_PROFILE;
#[cfg(target_os = "windows")]
const PLATFORM_PROFILE: AnimationProfile = WINDOWS_PROFILE;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const PLATFORM_PROFILE: AnimationProfile = OTHER_PROFILE;

/// The default end-to-end duration for the platform this build targets.
pub const PLATFORM_DEFAULT_DURATION_MS: u32 = PLATFORM_PROFILE.default_duration_ms;

/// The maximum normalized starting slope accepted when preserving momentum.
///
/// A cubic Hermite path with a slope in `[0, 3]` is monotonic and ends at rest.
const MAX_INITIAL_SLOPE: f64 = 3.0;

/// How the animation should be driven, derived from the user's configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimationParams {
    /// How long the movement takes, end to end.
    pub duration_ms: u32,
    /// How many frames per second the driver should aim to emit.
    pub fps: u32,
}

impl AnimationParams {
    /// The interval between emitted frames implied by [`AnimationParams::fps`].
    pub fn frame_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.fps.max(1)))
    }
}

/// A rigid rectangle moving on a finite cubic ease-out path.
#[derive(Debug, Clone, Copy)]
struct RectMotion {
    from: Rect,
    to: Rect,
    elapsed: f64,
    duration: f64,
    initial_slope: f64,
}

impl RectMotion {
    fn new(from: Rect, to: Rect, duration: f64, initial_slope: f64) -> Self {
        Self {
            from,
            to,
            elapsed: 0.0,
            duration,
            initial_slope,
        }
    }

    fn is_settled(&self) -> bool {
        self.from == self.to || self.elapsed >= self.duration
    }

    fn progress(&self) -> f64 {
        if self.duration <= 0.0 {
            return 1.0;
        }
        cubic_ease_out(
            (self.elapsed / self.duration).clamp(0.0, 1.0),
            self.initial_slope,
        )
    }

    fn progress_rate(&self) -> f64 {
        if self.duration <= 0.0 || self.elapsed >= self.duration {
            return 0.0;
        }
        cubic_ease_out_derivative(
            (self.elapsed / self.duration).clamp(0.0, 1.0),
            self.initial_slope,
        ) / self.duration
    }

    fn frame(&self) -> Rect {
        lerp_rect(self.from, self.to, self.progress())
    }

    fn velocity(&self) -> [f64; 4] {
        let rate = self.progress_rate();
        [
            (self.to.x - self.from.x) * rate,
            (self.to.y - self.from.y) * rate,
            (self.to.width - self.from.width) * rate,
            (self.to.height - self.from.height) * rate,
        ]
    }

    fn advance(&mut self, dt: Duration) {
        self.elapsed = (self.elapsed + dt.as_secs_f64()).min(self.duration);
    }

    fn retarget(&mut self, to: Rect, velocity: [f64; 4]) {
        let from = self.frame();
        let delta = [
            to.x - from.x,
            to.y - from.y,
            to.width - from.width,
            to.height - from.height,
        ];
        let distance_squared = delta.iter().map(|value| value * value).sum::<f64>();
        let projected_rate = if distance_squared > 0.0 {
            delta
                .iter()
                .zip(velocity)
                .map(|(distance, speed)| distance * speed)
                .sum::<f64>()
                / distance_squared
        } else {
            0.0
        };
        let initial_slope = (projected_rate * self.duration).clamp(0.0, MAX_INITIAL_SLOPE);

        *self = Self::new(from, to, self.duration, initial_slope);
    }
}

/// Cubic Hermite interpolation with a caller-supplied starting slope and a
/// zero ending slope. Slopes in `[0, 3]` stay between the endpoints.
fn cubic_ease_out(progress: f64, initial_slope: f64) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    let slope = initial_slope.clamp(0.0, MAX_INITIAL_SLOPE);
    (slope * (progress.powi(3) - 2.0 * progress.powi(2) + progress))
        + (-2.0 * progress.powi(3) + 3.0 * progress.powi(2))
}

fn cubic_ease_out_derivative(progress: f64, initial_slope: f64) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    let slope = initial_slope.clamp(0.0, MAX_INITIAL_SLOPE);
    slope * (3.0 * progress.powi(2) - 4.0 * progress + 1.0)
        + (-6.0 * progress.powi(2) + 6.0 * progress)
}

fn lerp(from: f64, to: f64, progress: f64) -> f64 {
    from + (to - from) * progress
}

fn lerp_rect(from: Rect, to: Rect, progress: f64) -> Rect {
    Rect::new(
        lerp(from.x, to.x, progress),
        lerp(from.y, to.y, progress),
        lerp(from.width, to.width, progress),
        lerp(from.height, to.height, progress),
    )
}

/// Animates a window's frame from one rectangle to another using the
/// compile-time-selected platform profile.
///
/// Construct with [`Animator::new`], then call [`Animator::step`] once per
/// frame with the elapsed time and apply the returned rectangle. Stop when
/// [`Animator::is_settled`] reports true; the final frame is exactly
/// [`Animator::target`].
#[derive(Debug, Clone, Copy)]
pub struct Animator {
    motion: RectMotion,
}

impl Animator {
    /// Starts an animation from `from` to `to`.
    pub fn new(from: Rect, to: Rect, params: AnimationParams) -> Self {
        Self::new_with_profile(from, to, params, PLATFORM_PROFILE)
    }

    fn new_with_profile(
        from: Rect,
        to: Rect,
        params: AnimationParams,
        profile: AnimationProfile,
    ) -> Self {
        let duration = f64::from(params.duration_ms.max(1)) / 1000.0;
        Self {
            motion: RectMotion::new(from, to, duration, profile.initial_slope),
        }
    }

    /// Points the animation at a new destination while preserving momentum
    /// along the new path when it can do so without crossing the destination.
    ///
    /// This is what a hotkey pressed mid-flight goes through: the window keeps
    /// moving smoothly instead of stopping dead and restarting from zero.
    pub fn retarget(&mut self, to: Rect) {
        let velocity = self.motion.velocity();
        self.motion.retarget(to, velocity);
    }

    /// The rectangle this animation is heading for.
    pub fn target(&self) -> Rect {
        self.motion.to
    }

    /// True once the finite path has reached its end.
    pub fn is_settled(&self) -> bool {
        self.motion.is_settled()
    }

    /// Advances the animation by `dt` of wall-clock time and returns the frame
    /// to apply.
    ///
    /// Once settled this returns [`Animator::target`] verbatim, so the last
    /// frame the caller applies is the exact rectangle the engine planned.
    pub fn step(&mut self, dt: Duration) -> Rect {
        if self.is_settled() {
            return self.target();
        }

        self.motion.advance(dt);
        if self.is_settled() {
            self.target()
        } else {
            self.motion.frame()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMS: AnimationParams = AnimationParams {
        duration_ms: 220,
        fps: 90,
    };

    fn run(animator: &mut Animator, frame: Duration) -> Vec<Rect> {
        let mut frames = Vec::new();
        for _ in 0..10_000 {
            frames.push(animator.step(frame));
            if animator.is_settled() {
                return frames;
            }
        }
        panic!("animator failed to settle after 10000 frames");
    }

    fn assert_rigid_and_monotonic(
        from: Rect,
        to: Rect,
        params: AnimationParams,
        profile: AnimationProfile,
    ) {
        let mut animator = Animator::new_with_profile(from, to, params, profile);
        for frame in run(&mut animator, Duration::from_millis(11)) {
            let progress = if to.x != from.x {
                (frame.x - from.x) / (to.x - from.x)
            } else if to.y != from.y {
                (frame.y - from.y) / (to.y - from.y)
            } else {
                (frame.width - from.width) / (to.width - from.width)
            };

            assert!((0.0..=1.0).contains(&progress), "overshot: {frame:?}");
            assert!(
                frame.approx_eq(&lerp_rect(from, to, progress), 1e-9),
                "frame was not rigid: {frame:?}"
            );
        }
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn platform_profiles_are_rigid_and_tuned_for_each_desktop() {
        assert_eq!(MACOS_PROFILE.default_duration_ms, 250);
        assert_eq!(WINDOWS_PROFILE.default_duration_ms, 220);
        assert!(MACOS_PROFILE.initial_slope < WINDOWS_PROFILE.initial_slope);
        assert_eq!(
            PLATFORM_DEFAULT_DURATION_MS,
            PLATFORM_PROFILE.default_duration_ms
        );

        let from = Rect::new(0.0, 0.0, 400.0, 300.0);
        let to = Rect::new(800.0, 600.0, 800.0, 500.0);
        assert_rigid_and_monotonic(from, to, PARAMS, MACOS_PROFILE);
        assert_rigid_and_monotonic(from, to, PARAMS, WINDOWS_PROFILE);
    }

    #[test]
    fn easing_stays_between_endpoints_with_rigid_dimensions() {
        let from = Rect::new(0.0, 0.0, 400.0, 300.0);
        let to = Rect::new(800.0, 600.0, 800.0, 500.0);

        for profile in [MACOS_PROFILE, WINDOWS_PROFILE] {
            assert_rigid_and_monotonic(from, to, PARAMS, profile);
        }
    }

    #[test]
    fn retargeting_keeps_forward_momentum_when_direction_matches() {
        let from = Rect::new(0.0, 0.0, 400.0, 300.0);
        let mut animator = Animator::new_with_profile(
            from,
            Rect::new(800.0, 0.0, 400.0, 300.0),
            PARAMS,
            WINDOWS_PROFILE,
        );
        for _ in 0..4 {
            animator.step(Duration::from_millis(11));
        }

        animator.retarget(Rect::new(1200.0, 0.0, 400.0, 300.0));

        assert!(animator.motion.initial_slope > 0.0);
        assert!(animator.step(Duration::from_millis(11)).x > from.x);
    }

    #[test]
    fn retargeting_reaches_the_new_target_exactly() {
        let from = Rect::new(0.0, 0.0, 400.0, 300.0);
        let to = Rect::new(1200.0, 500.0, 600.0, 400.0);
        let mut animator = Animator::new(from, Rect::new(800.0, 0.0, 400.0, 300.0), PARAMS);
        for _ in 0..3 {
            animator.step(Duration::from_millis(11));
        }

        animator.retarget(to);
        let frames = run(&mut animator, Duration::from_millis(11));

        assert_eq!(*frames.last().unwrap(), to);
    }

    #[test]
    fn reversing_direction_stays_monotonic_without_crossing_the_target() {
        let from = Rect::new(0.0, 0.0, 400.0, 300.0);
        let mut animator = Animator::new(from, Rect::new(800.0, 0.0, 400.0, 300.0), PARAMS);
        for _ in 0..4 {
            animator.step(Duration::from_millis(11));
        }

        let current = animator.motion.frame();
        let to = Rect::new(-400.0, 0.0, 400.0, 300.0);
        animator.retarget(to);
        for frame in run(&mut animator, Duration::from_millis(11)) {
            let progress = (frame.x - current.x) / (to.x - current.x);
            assert!((0.0..=1.0).contains(&progress), "overshot: {frame:?}");
            assert!(frame.approx_eq(&lerp_rect(current, to, progress), 1e-9));
        }
    }

    #[test]
    fn settled_frames_are_exact_and_repeatable() {
        let to = Rect::new(100.0, 100.0, 200.0, 200.0);
        let mut animator = Animator::new(Rect::new(0.0, 0.0, 200.0, 200.0), to, PARAMS);
        run(&mut animator, Duration::from_millis(11));

        assert_eq!(animator.step(Duration::from_millis(11)), to);
        assert_eq!(animator.step(Duration::from_millis(11)), to);
    }

    #[test]
    fn frame_rate_does_not_change_the_path() {
        let from = Rect::new(0.0, 0.0, 400.0, 300.0);
        let to = Rect::new(960.0, 540.0, 800.0, 600.0);
        let mut fast = Animator::new(from, to, PARAMS);
        let mut slow = Animator::new(from, to, PARAMS);

        for _ in 0..12 {
            fast.step(Duration::from_millis(10));
        }
        for _ in 0..6 {
            slow.step(Duration::from_millis(20));
        }

        assert!(fast
            .step(Duration::ZERO)
            .approx_eq(&slow.step(Duration::ZERO), 1e-9));
    }

    #[test]
    fn a_zero_length_move_settles_immediately() {
        let rect = Rect::new(100.0, 100.0, 400.0, 300.0);
        let animator = Animator::new(rect, rect, PARAMS);

        assert!(animator.is_settled());
        assert_eq!(animator.target(), rect);
    }

    #[test]
    fn frame_interval_follows_fps() {
        let params = AnimationParams {
            duration_ms: 140,
            fps: 100,
        };
        assert_eq!(params.frame_interval(), Duration::from_millis(10));

        let broken = AnimationParams {
            duration_ms: 140,
            fps: 0,
        };
        assert_eq!(broken.frame_interval(), Duration::from_secs(1));
    }
}
