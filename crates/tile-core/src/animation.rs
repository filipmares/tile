//! The spring animator that makes a snap *flow* instead of teleport.
//!
//! This module is deliberately pure: no clock, no threads, no I/O. The caller
//! supplies the elapsed time for each step and receives a rectangle back, which
//! is what lets the whole feel of the animation be unit tested on any host —
//! the same reason the rest of `tile-core` exists.
//!
//! # Why springs rather than a duration-based easing curve
//!
//! Tile's hotkeys are pressed in bursts: half-left, then half-left again to
//! cycle, then top-half. With an easing curve, a press that arrives mid-flight
//! must either be queued (the window visibly walks through every intermediate
//! layout) or restarted from zero velocity (a visible stutter). A spring has
//! state — position *and* velocity — so [`Animator::retarget`] can swap the
//! destination while keeping the momentum the window already has, and the
//! motion simply bends towards the new target. That property is the entire
//! reason for the choice.
//!
//! # Why one spring per edge
//!
//! A single spring driving the rectangle as a rigid box slides mechanically.
//! Giving each of the four edges its own spring, and making the edge that moves
//! *in the direction of travel* stiffer than the one behind it, means the
//! leading edge arrives first: the window stretches towards its destination and
//! the trailing edge catches up a beat later. That asymmetry is what reads as
//! "liquid" rather than "translated".

use std::time::Duration;

use crate::geometry::Rect;

/// Measured settle time, in milliseconds, of the springs below at a time scale
/// of 1.0, for the representative snap (a half-screen move on a 1920×1080
/// display).
///
/// This is what makes [`AnimationParams::duration_ms`] mean something: the
/// integrator advances at `NATURAL_SETTLE_MS / duration_ms`, so a configured
/// 450 ms really does finish in about 450 ms. Change the spring constants and
/// this number has to be re-measured, which
/// `the_configured_duration_is_the_real_settle_time` enforces.
///
/// It is approximate by nature — the settle test uses an absolute half-pixel
/// threshold, so a longer move takes marginally longer to satisfy it (a 4K
/// half snap runs about 8% over, a short nudge well under).
pub const NATURAL_SETTLE_MS: f64 = 513.0;

/// Stiffness and damping of the edge that leads the movement.
///
/// Damping ratio ≈ 0.76. The *ratio* is the springiness dial, independently of
/// how fast the whole thing runs: this one leaves a soft overshoot of about
/// 1.7% of the distance travelled — 17 px on a half-screen snap — that eases
/// back rather than bouncing. Pushing it towards 1.0 removes the overshoot and
/// the motion turns mechanical; dropping it much below 0.7 makes a window
/// visibly sail past the screen edge and wobble back.
const LEADING: (f64, f64) = (700.0, 40.0);

/// Stiffness and damping of the edge that trails the movement.
///
/// Damping ratio ≈ 0.85 — a little more settled than the leading edge, so the
/// window stops elongating before it stops moving. Less than half the leading
/// stiffness, and that gap is the whole effect: on a half-screen snap the
/// window stretches about 260 px past its final width before the trailing edge
/// closes it up. Constants close to each other (the first cut used 170/22
/// against 145/20, a ratio of 1.17) produce a stretch too small to see, which
/// is a mechanical slide with extra arithmetic.
const TRAILING: (f64, f64) = (330.0, 31.0);

/// Size of one integration sub-step, in seconds.
///
/// The springs are integrated at this fixed rate regardless of how much real
/// time a frame actually took. Explicit Euler is only stable for a step small
/// relative to the spring period, and pacing jitter (a frame that took 40 ms
/// because the target app stalled) would otherwise change the trajectory. A
/// fixed sub-step makes the motion identical whether it is driven at 30 or 120
/// frames per second, and makes the unit tests below exactly reproducible.
const FIXED_STEP: f64 = 1.0 / 240.0;

/// An edge is settled once it is within this many pixels of its target.
///
/// Half a pixel: below the granularity any window backend can actually render.
const POSITION_EPSILON: f64 = 0.5;

/// ...and moving slower than this, in pixels per second.
///
/// Position alone is not enough: an edge crossing its target at speed is
/// momentarily "in position" while still needing to decelerate and come back.
const VELOCITY_EPSILON: f64 = 8.0;

/// Hard ceiling on simulated time, as a multiple of the natural settle time.
///
/// A spring approaches its target asymptotically, and a retarget can restart
/// the approach from an awkward state. This budget guarantees termination:
/// once it is exhausted the animator reports itself settled and emits the
/// exact target, so the frame pump can never spin forever no matter what it is
/// fed.
///
/// Three times the natural settle, not one: the multiplier has to clear the
/// *slowest* real move, not the representative one [`NATURAL_SETTLE_MS`] is
/// calibrated on. A 4K half snap settles around 554 ms naturally, so a tighter
/// budget would truncate it and make the window visibly jump the last few
/// pixels — which is exactly what the first cut of this module did.
const BUDGET_MULTIPLIER: f64 = 3.0;

/// How the animation should be driven, derived from the user's configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimationParams {
    /// Roughly how long the movement takes, end to end.
    pub duration_ms: u32,
    /// How many frames per second the driver should aim to emit. Purely
    /// informational to the animator itself, which is driven by the `dt` it is
    /// handed, but it lives here so the whole animation configuration travels
    /// as one value.
    pub fps: u32,
}

impl AnimationParams {
    /// Factor applied to elapsed time so a shorter configured duration runs the
    /// same simulation faster, and a longer one slower.
    fn time_scale(&self) -> f64 {
        let duration = f64::from(self.duration_ms).max(1.0);
        NATURAL_SETTLE_MS / duration
    }

    /// Total simulated time after which the animation is forced to settle.
    fn budget(&self) -> f64 {
        BUDGET_MULTIPLIER * NATURAL_SETTLE_MS / 1000.0
    }

    /// The interval between emitted frames implied by [`AnimationParams::fps`].
    pub fn frame_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.fps.max(1)))
    }
}

/// One damped spring: `a = k*(target - pos) - c*vel`.
#[derive(Debug, Clone, Copy)]
struct Spring {
    pos: f64,
    vel: f64,
    target: f64,
    /// Stiffness: how hard the spring pulls towards the target.
    k: f64,
    /// Damping: how quickly the motion bleeds off. Without it the spring
    /// oscillates forever.
    c: f64,
}

impl Spring {
    fn new(pos: f64, target: f64) -> Self {
        Self {
            pos,
            vel: 0.0,
            target,
            // Overwritten by `set_role` before the first step; a sane default
            // keeps the type valid in its own right.
            k: TRAILING.0,
            c: TRAILING.1,
        }
    }

    /// Assigns the leading or trailing constants. Called on construction and
    /// again on every retarget, because a new destination can flip which edge
    /// is in front.
    fn set_role(&mut self, leading: bool) {
        let (k, c) = if leading { LEADING } else { TRAILING };
        self.k = k;
        self.c = c;
    }

    /// Advances the spring by one fixed sub-step (semi-implicit Euler: the
    /// velocity is updated first and the new velocity moves the position,
    /// which is markedly more stable than the explicit form).
    fn integrate(&mut self, dt: f64) {
        let acceleration = self.k * (self.target - self.pos) - self.c * self.vel;
        self.vel += acceleration * dt;
        self.pos += self.vel * dt;
    }

    fn settled(&self) -> bool {
        (self.target - self.pos).abs() <= POSITION_EPSILON && self.vel.abs() <= VELOCITY_EPSILON
    }
}

/// Animates a window's frame from one rectangle to another, one edge at a time.
///
/// Construct with [`Animator::new`], then call [`Animator::step`] once per
/// frame with the elapsed time and apply the returned rectangle to the window.
/// Stop when [`Animator::is_settled`] reports true — the rectangle returned by
/// that final step is exactly [`Animator::target`].
#[derive(Debug, Clone)]
pub struct Animator {
    left: Spring,
    top: Spring,
    right: Spring,
    bottom: Spring,
    params: AnimationParams,
    /// Sub-step remainder carried between calls, so a caller whose frames do
    /// not divide evenly into [`FIXED_STEP`] still integrates exactly the time
    /// that elapsed rather than repeatedly truncating it away.
    carry: f64,
    /// Simulated time consumed so far, checked against [`AnimationParams::budget`].
    elapsed: f64,
    /// Latched once the budget runs out, so the animator keeps reporting itself
    /// settled even if it is stepped again.
    exhausted: bool,
}

impl Animator {
    /// Starts an animation from `from` to `to`, both at rest.
    pub fn new(from: Rect, to: Rect, params: AnimationParams) -> Self {
        let mut animator = Self {
            left: Spring::new(from.x, to.x),
            top: Spring::new(from.y, to.y),
            right: Spring::new(from.max_x(), to.max_x()),
            bottom: Spring::new(from.max_y(), to.max_y()),
            params,
            carry: 0.0,
            elapsed: 0.0,
            exhausted: false,
        };
        animator.assign_roles();
        animator
    }

    /// Points the animation at a new destination **without touching velocity**.
    ///
    /// This is what a hotkey pressed mid-flight goes through: the window keeps
    /// the momentum it already had and bends towards the new frame instead of
    /// stopping dead and starting again. The time budget is also refreshed,
    /// since this is a new movement rather than a continuation of the old one.
    pub fn retarget(&mut self, to: Rect) {
        self.left.target = to.x;
        self.top.target = to.y;
        self.right.target = to.max_x();
        self.bottom.target = to.max_y();
        self.elapsed = 0.0;
        self.exhausted = false;
        self.assign_roles();
    }

    /// Decides, per axis, which edge is leading and which is trailing.
    ///
    /// The leading edge is the one on the side the rectangle is heading
    /// towards: moving right, the right edge leads. A pure resize with a fixed
    /// left edge therefore leads with the right edge, which is also the edge
    /// the user is watching.
    fn assign_roles(&mut self) {
        let horizontal = (self.left.target - self.left.pos) + (self.right.target - self.right.pos);
        let vertical = (self.top.target - self.top.pos) + (self.bottom.target - self.bottom.pos);

        // A tie (a symmetric grow or shrink about the centre) has no leading
        // edge; treating the far edge as leading keeps the behaviour defined
        // and matches the "moving right" case at the limit.
        self.right.set_role(horizontal >= 0.0);
        self.left.set_role(horizontal < 0.0);
        self.bottom.set_role(vertical >= 0.0);
        self.top.set_role(vertical < 0.0);
    }

    /// The rectangle this animation is heading for.
    pub fn target(&self) -> Rect {
        Rect::new(
            self.left.target,
            self.top.target,
            self.right.target - self.left.target,
            self.bottom.target - self.top.target,
        )
    }

    /// True once every edge is close enough and slow enough — or the time
    /// budget has run out.
    pub fn is_settled(&self) -> bool {
        self.exhausted
            || (self.left.settled()
                && self.top.settled()
                && self.right.settled()
                && self.bottom.settled())
    }

    /// Advances the animation by `dt` of wall-clock time and returns the frame
    /// to apply.
    ///
    /// Once settled this returns [`Animator::target`] verbatim, so the last
    /// frame the caller applies is the exact rectangle the engine planned —
    /// never a fraction of a pixel off.
    pub fn step(&mut self, dt: Duration) -> Rect {
        if self.is_settled() {
            return self.target();
        }

        let scaled = dt.as_secs_f64() * self.params.time_scale();
        // Guard against a pathological `dt` (a debugger pause, a suspended
        // laptop) turning into thousands of sub-steps: the budget check below
        // would end the animation anyway, so clamp up front.
        let budget = self.params.budget();
        self.carry += scaled.min(budget);

        while self.carry >= FIXED_STEP {
            self.carry -= FIXED_STEP;
            self.elapsed += FIXED_STEP;
            self.left.integrate(FIXED_STEP);
            self.top.integrate(FIXED_STEP);
            self.right.integrate(FIXED_STEP);
            self.bottom.integrate(FIXED_STEP);

            if self.elapsed >= budget {
                self.exhausted = true;
                break;
            }
        }

        if self.is_settled() {
            return self.target();
        }

        Rect::new(
            self.left.pos,
            self.top.pos,
            // An overshooting leading edge can momentarily cross the trailing
            // one on a very short movement. A negative width is meaningless to
            // every backend, so clamp rather than hand one out.
            (self.right.pos - self.left.pos).max(0.0),
            (self.bottom.pos - self.top.pos).max(0.0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMS: AnimationParams = AnimationParams {
        duration_ms: 450,
        fps: 90,
    };

    /// Drives an animator at a fixed frame rate until it settles, returning
    /// every frame it emitted. Panics rather than looping forever if the
    /// animator refuses to settle, which is the property most of these tests
    /// ultimately rest on.
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

    #[test]
    fn the_leading_edge_overshoots_softly() {
        // Springiness is the point, so this is bounded on *both* sides.
        //
        // Too little overshoot and the motion is a mechanical slide — an
        // earlier revision damped it to 0.1% of travel and lost the character
        // entirely. Too much and a window sails visibly off the screen edge
        // before wobbling back, which reads as a glitch rather than as
        // liquid. About 1.7% is the intended feel.
        let from = Rect::new(960.0, 0.0, 960.0, 1080.0);
        let to = Rect::new(0.0, 0.0, 960.0, 1080.0);
        let travel = from.x - to.x;
        let mut animator = Animator::new(from, to, PARAMS);

        let frames = run(&mut animator, Duration::from_millis(4));
        let furthest = frames.iter().fold(f64::MAX, |acc, r| acc.min(r.x));
        let overshoot = to.x - furthest;

        assert!(
            overshoot >= travel * 0.005,
            "overshoot was only {overshoot:.1}px — the spring has been damped flat"
        );
        assert!(
            overshoot <= travel * 0.03,
            "overshoot was {overshoot:.1}px — too loose, the window sails off screen"
        );
    }

    #[test]
    fn the_configured_duration_is_the_real_settle_time() {
        // `durationMs` is only meaningful if it matches reality, and it only
        // does so while `NATURAL_SETTLE_MS` matches the spring constants. This
        // is the test that catches a constant changed without recalibrating —
        // without it, "180 ms" silently drifted to 567 ms in an earlier
        // revision of this module.
        let frame = Duration::from_millis(11);
        for duration_ms in [80, 180, 340, 450, 700] {
            let params = AnimationParams {
                duration_ms,
                fps: 90,
            };
            // The representative snap `NATURAL_SETTLE_MS` is calibrated on: a
            // half-screen move on a 1920x1080 display.
            let mut animator = Animator::new(
                Rect::new(960.0, 0.0, 960.0, 1080.0),
                Rect::new(0.0, 0.0, 960.0, 1080.0),
                params,
            );

            let frames = run(&mut animator, frame);
            let actual_ms = frames.len() as f64 * frame.as_secs_f64() * 1000.0;
            let nominal = f64::from(duration_ms);

            // Within 15%, plus one frame of quantisation: the settle test uses
            // an absolute pixel threshold, so it can never be exact.
            let tolerance = nominal * 0.15 + 11.0;
            assert!(
                (actual_ms - nominal).abs() <= tolerance,
                "durationMs {duration_ms} settled in {actual_ms:.0}ms, outside ±{tolerance:.0}ms"
            );
        }
    }

    #[test]
    fn the_stretch_is_large_enough_to_see() {
        // The per-edge springs only earn their complexity if the lag between
        // the leading and trailing edge is actually visible. Guards against
        // someone tuning the two sets closer together and quietly turning the
        // animation back into a rigid slide.
        let mut animator = Animator::new(
            Rect::new(960.0, 0.0, 960.0, 1080.0),
            Rect::new(0.0, 0.0, 960.0, 1080.0),
            PARAMS,
        );

        let frames = run(&mut animator, Duration::from_millis(11));
        let widest = frames.iter().fold(0.0_f64, |acc, r| acc.max(r.width));

        assert!(
            widest - 960.0 >= 100.0,
            "stretch was only {:.1}px on a 960px snap",
            widest - 960.0
        );
    }

    #[test]
    fn the_budget_clears_the_slowest_real_move() {
        // `NATURAL_SETTLE_MS` is calibrated on a 1080p half snap, but the
        // absolute settle threshold makes larger moves take longer. A 4K half
        // snap is the worst case Tile realistically sees, and it must settle on
        // its own merits rather than being truncated by the budget.
        let mut animator = Animator::new(
            Rect::new(1920.0, 0.0, 1920.0, 2160.0),
            Rect::new(0.0, 0.0, 1920.0, 2160.0),
            PARAMS,
        );

        let mut frames = 0;
        let frame = Duration::from_millis(4);
        while !animator.is_settled() && frames < 10_000 {
            animator.step(frame);
            frames += 1;
        }

        assert!(!animator.exhausted, "the time budget truncated a real move");
    }

    #[test]
    fn settles_on_the_exact_target() {
        let from = Rect::new(100.0, 100.0, 400.0, 300.0);
        let to = Rect::new(0.0, 0.0, 960.0, 1040.0);
        let mut animator = Animator::new(from, to, PARAMS);

        let frames = run(&mut animator, Duration::from_millis(11));

        assert!(animator.is_settled());
        // The exact planned rectangle, not merely something near it: a window
        // left half a pixel short would break flush-edge tiling.
        assert_eq!(*frames.last().unwrap(), to);
    }

    #[test]
    fn the_first_frame_moves_but_does_not_jump_to_the_target() {
        let from = Rect::new(0.0, 0.0, 400.0, 300.0);
        let to = Rect::new(800.0, 0.0, 400.0, 300.0);
        let mut animator = Animator::new(from, to, PARAMS);

        let first = animator.step(Duration::from_millis(11));

        assert!(first.x > from.x, "expected movement, got {first:?}");
        assert!(first.x < to.x, "expected interpolation, got {first:?}");
    }

    #[test]
    fn the_leading_edge_arrives_before_the_trailing_one() {
        // Moving right: the right edge leads, so partway through the animation
        // the window is stretched — wider than it started and than it ends.
        let from = Rect::new(0.0, 0.0, 400.0, 300.0);
        let to = Rect::new(800.0, 0.0, 400.0, 300.0);
        let mut animator = Animator::new(from, to, PARAMS);

        let frames = run(&mut animator, Duration::from_millis(11));
        let widest = frames.iter().fold(0.0_f64, |acc, r| acc.max(r.width));

        assert!(
            widest > 400.0,
            "expected the window to stretch, widest was {widest}"
        );
    }

    #[test]
    fn retarget_preserves_velocity() {
        let from = Rect::new(0.0, 0.0, 400.0, 300.0);
        let mut animator = Animator::new(from, Rect::new(800.0, 0.0, 400.0, 300.0), PARAMS);
        for _ in 0..4 {
            animator.step(Duration::from_millis(11));
        }
        let moving = animator.left.vel;
        assert!(moving > 0.0, "precondition: the window should be moving");

        // Retargeting further along the same direction must not zero the
        // momentum — that stutter is exactly what springs were chosen to
        // avoid.
        animator.retarget(Rect::new(1200.0, 0.0, 400.0, 300.0));

        assert_eq!(animator.left.vel, moving);
        assert_eq!(animator.target(), Rect::new(1200.0, 0.0, 400.0, 300.0));
    }

    #[test]
    fn retarget_reaches_the_new_target_exactly() {
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
    fn retargeting_back_to_the_current_frame_still_settles() {
        // The degenerate case a repeated hotkey produces: the window is already
        // heading somewhere and is told to stay put. It must decelerate and
        // stop rather than ring.
        let from = Rect::new(0.0, 0.0, 400.0, 300.0);
        let mut animator = Animator::new(from, Rect::new(800.0, 0.0, 400.0, 300.0), PARAMS);
        for _ in 0..3 {
            animator.step(Duration::from_millis(11));
        }

        animator.retarget(from);
        let frames = run(&mut animator, Duration::from_millis(11));

        assert_eq!(*frames.last().unwrap(), from);
    }

    #[test]
    fn frame_rate_does_not_change_the_trajectory() {
        let from = Rect::new(0.0, 0.0, 400.0, 300.0);
        let to = Rect::new(960.0, 540.0, 800.0, 600.0);

        // Half the frame rate, twice the step: the fixed-timestep accumulator
        // must land both runs on the same simulated state, so the animation
        // looks the same on a 60 Hz and a 120 Hz driver.
        let mut fast = Animator::new(from, to, PARAMS);
        for _ in 0..12 {
            fast.step(Duration::from_millis(10));
        }
        let mut slow = Animator::new(from, to, PARAMS);
        for _ in 0..6 {
            slow.step(Duration::from_millis(20));
        }

        assert!(
            fast.step(Duration::ZERO)
                .approx_eq(&slow.step(Duration::ZERO), 1e-9),
            "{:?} vs {:?}",
            fast.step(Duration::ZERO),
            slow.step(Duration::ZERO)
        );
    }

    #[test]
    fn stepping_is_deterministic() {
        let from = Rect::new(10.0, 20.0, 300.0, 200.0);
        let to = Rect::new(400.0, 300.0, 500.0, 450.0);

        let mut a = Animator::new(from, to, PARAMS);
        let mut b = Animator::new(from, to, PARAMS);

        assert_eq!(
            run(&mut a, Duration::from_millis(11)),
            run(&mut b, Duration::from_millis(11))
        );
    }

    #[test]
    fn a_short_duration_settles_in_fewer_frames_than_a_long_one() {
        let from = Rect::new(0.0, 0.0, 400.0, 300.0);
        let to = Rect::new(800.0, 600.0, 400.0, 300.0);

        let mut quick = Animator::new(
            from,
            to,
            AnimationParams {
                duration_ms: 60,
                fps: 90,
            },
        );
        let mut slow = Animator::new(
            from,
            to,
            AnimationParams {
                duration_ms: 400,
                fps: 90,
            },
        );

        let quick_frames = run(&mut quick, Duration::from_millis(11)).len();
        let slow_frames = run(&mut slow, Duration::from_millis(11)).len();

        assert!(
            quick_frames < slow_frames,
            "{quick_frames} frames at 60ms vs {slow_frames} at 400ms"
        );
    }

    #[test]
    fn a_tiny_step_never_starves_the_animation() {
        // Sub-`FIXED_STEP` frames must accumulate rather than be truncated
        // away, or a fast driver would leave the window frozen forever.
        let from = Rect::new(0.0, 0.0, 400.0, 300.0);
        let to = Rect::new(800.0, 0.0, 400.0, 300.0);
        let mut animator = Animator::new(from, to, PARAMS);

        let frames = run(&mut animator, Duration::from_micros(500));

        assert_eq!(*frames.last().unwrap(), to);
    }

    #[test]
    fn the_time_budget_forces_a_settle() {
        // Deliberately hostile: a duration long enough that the springs are
        // nowhere near their target, driven with huge steps. The budget must
        // still end the animation, on the exact target.
        let from = Rect::new(0.0, 0.0, 400.0, 300.0);
        let to = Rect::new(5000.0, 4000.0, 400.0, 300.0);
        let mut animator = Animator::new(
            from,
            to,
            AnimationParams {
                duration_ms: 1000,
                fps: 15,
            },
        );

        let frames = run(&mut animator, Duration::from_secs(5));

        assert!(animator.is_settled());
        assert_eq!(*frames.last().unwrap(), to);
    }

    #[test]
    fn a_settled_animator_keeps_reporting_the_target() {
        let to = Rect::new(100.0, 100.0, 200.0, 200.0);
        let mut animator = Animator::new(Rect::new(0.0, 0.0, 200.0, 200.0), to, PARAMS);
        run(&mut animator, Duration::from_millis(11));

        assert_eq!(animator.step(Duration::from_millis(11)), to);
        assert_eq!(animator.step(Duration::from_millis(11)), to);
    }

    #[test]
    fn a_zero_length_move_settles_immediately() {
        let rect = Rect::new(100.0, 100.0, 400.0, 300.0);
        let animator = Animator::new(rect, rect, PARAMS);

        assert!(animator.is_settled());
        assert_eq!(animator.target(), rect);
    }

    #[test]
    fn frames_never_have_a_negative_size() {
        // A very short movement is where an overshooting leading edge could
        // cross the trailing one.
        let from = Rect::new(0.0, 0.0, 4.0, 4.0);
        let to = Rect::new(2.0, 2.0, 2.0, 2.0);
        let mut animator = Animator::new(from, to, PARAMS);

        for frame in run(&mut animator, Duration::from_millis(11)) {
            assert!(frame.width >= 0.0 && frame.height >= 0.0, "{frame:?}");
        }
    }

    #[test]
    fn frame_interval_follows_fps() {
        let params = AnimationParams {
            duration_ms: 140,
            fps: 100,
        };
        assert_eq!(params.frame_interval(), Duration::from_millis(10));

        // A zero fps would divide by zero; it is clamped instead so a
        // hand-edited config cannot panic the frame pump.
        let broken = AnimationParams {
            duration_ms: 140,
            fps: 0,
        };
        assert_eq!(broken.frame_interval(), Duration::from_secs(1));
    }
}
