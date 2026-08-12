//! A minimal time-based rate limiter.
//!
//! Used to ensure a held-down hotkey that keeps failing with
//! `PermissionDenied` cannot spawn a flood of system dialogs: the limiter only
//! allows one event through per cooldown window.

use std::time::{Duration, Instant};

/// Allows an action at most once per `cooldown`.
#[derive(Debug)]
pub struct RateLimiter {
    cooldown: Duration,
    last: Option<Instant>,
}

impl RateLimiter {
    pub fn new(cooldown: Duration) -> Self {
        Self {
            cooldown,
            last: None,
        }
    }

    /// Returns `true` and arms the cooldown if enough time has elapsed since
    /// the previous allowed call; otherwise returns `false`.
    pub fn allow(&mut self) -> bool {
        self.allow_at(Instant::now())
    }

    /// Testable core of [`RateLimiter::allow`] with an injected clock reading.
    fn allow_at(&mut self, now: Instant) -> bool {
        let ready = match self.last {
            Some(last) => now.duration_since(last) >= self.cooldown,
            None => true,
        };
        if ready {
            self.last = Some(now);
        }
        ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_call_is_always_allowed() {
        let mut limiter = RateLimiter::new(Duration::from_secs(5));
        assert!(limiter.allow());
    }

    #[test]
    fn a_second_immediate_call_is_blocked() {
        let mut limiter = RateLimiter::new(Duration::from_secs(5));
        let t0 = Instant::now();
        assert!(limiter.allow_at(t0));
        assert!(!limiter.allow_at(t0 + Duration::from_millis(100)));
    }

    #[test]
    fn a_call_after_the_cooldown_is_allowed_again() {
        let mut limiter = RateLimiter::new(Duration::from_secs(5));
        let t0 = Instant::now();
        assert!(limiter.allow_at(t0));
        assert!(!limiter.allow_at(t0 + Duration::from_secs(4)));
        assert!(limiter.allow_at(t0 + Duration::from_secs(5)));
    }

    #[test]
    fn each_allowed_call_re_arms_the_window() {
        let mut limiter = RateLimiter::new(Duration::from_secs(2));
        let t0 = Instant::now();
        assert!(limiter.allow_at(t0));
        assert!(limiter.allow_at(t0 + Duration::from_secs(2)));
        // Only 1s after the last *allowed* call, so still blocked.
        assert!(!limiter.allow_at(t0 + Duration::from_secs(3)));
    }
}
