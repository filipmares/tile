//! User configuration: key bindings and behaviour, persisted as JSON.

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::fmt;

use crate::action::WindowAction;
use crate::animation::AnimationParams;
use crate::geometry::Rect;
use crate::hotkey::{Hotkey, KeyCode, Modifiers};

/// Name of the config file inside the platform config directory.
pub const CONFIG_FILE_NAME: &str = "config.json";

/// Errors produced while loading or validating configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to parse configuration: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
}

/// A conflict between two actions bound to the same hotkey.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    pub hotkey: Hotkey,
    pub actions: Vec<WindowAction>,
}

/// Which edges of a target rectangle are shared with a neighbouring window
/// rather than lying against the screen edge.
///
/// A shared edge receives *half* the window gap, so two adjacent windows end
/// up separated by exactly one window gap. A non-shared (screen) edge receives
/// the appropriate per-side screen-edge gap instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SharedEdges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

/// The gap model: a single window gap between adjacent tiled windows plus
/// independently configurable per-side screen-edge gaps.
///
/// This generalises the original scalar `gap`, which insets every side
/// uniformly and therefore cannot express "one gap between two windows but a
/// different gap against the screen edge". A legacy scalar config still loads —
/// see the custom [`Deserialize`] impl below.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gaps {
    /// Space between two adjacent tiled windows. Each shares half of it.
    pub window: f64,
    pub edge_top: f64,
    pub edge_bottom: f64,
    pub edge_left: f64,
    pub edge_right: f64,
    /// Drop the top screen-edge gap entirely (Rectangle's `skipGapTopEdge`).
    pub skip_top_edge: bool,
    /// Apply screen-edge gaps only on the primary display (Rectangle's
    /// `screenEdgeGapsOnMainScreenOnly`). The window gap always applies.
    pub main_screen_only: bool,
}

impl Default for Gaps {
    fn default() -> Self {
        Self {
            window: 0.0,
            edge_top: 0.0,
            edge_bottom: 0.0,
            edge_left: 0.0,
            edge_right: 0.0,
            skip_top_edge: false,
            main_screen_only: false,
        }
    }
}

impl Gaps {
    /// A uniform gap on every side and between windows, matching the legacy
    /// scalar `gap` semantics for migration.
    pub fn uniform(value: f64) -> Self {
        Self {
            window: value,
            edge_top: value,
            edge_bottom: value,
            edge_left: value,
            edge_right: value,
            skip_top_edge: false,
            main_screen_only: false,
        }
    }

    /// Clamps every gap into `[0, MAX_GAP]`, mapping negative or non-finite
    /// values to zero so a hand-edited or older config can never shrink a
    /// window away.
    pub fn normalize(&mut self) {
        for value in [
            &mut self.window,
            &mut self.edge_top,
            &mut self.edge_bottom,
            &mut self.edge_left,
            &mut self.edge_right,
        ] {
            if !value.is_finite() || *value < 0.0 {
                *value = 0.0;
            }
            *value = value.min(MAX_GAP);
        }
    }

    fn screen_edge(&self, value: f64, main_screen: bool) -> f64 {
        if self.main_screen_only && !main_screen {
            0.0
        } else {
            value
        }
    }

    fn edge_top_effective(&self, main_screen: bool) -> f64 {
        if self.skip_top_edge {
            0.0
        } else {
            self.screen_edge(self.edge_top, main_screen)
        }
    }

    /// Insets `rect` per edge: shared edges get half the window gap, screen
    /// edges get the relevant per-side screen-edge gap. Never grows the window.
    pub fn apply(&self, rect: Rect, shared: SharedEdges, main_screen: bool) -> Rect {
        let half = (self.window / 2.0).max(0.0);
        let left = if shared.left {
            half
        } else {
            self.screen_edge(self.edge_left, main_screen)
        };
        let right = if shared.right {
            half
        } else {
            self.screen_edge(self.edge_right, main_screen)
        };
        let top = if shared.top {
            half
        } else {
            self.edge_top_effective(main_screen)
        };
        let bottom = if shared.bottom {
            half
        } else {
            self.screen_edge(self.edge_bottom, main_screen)
        };
        rect.inset_edges(left, top, right, bottom)
    }
}

/// Accepts either a legacy scalar (`"gap": 24`) or the new object form
/// (`"gap": { "window": 8, "edgeTop": 0, ... }`), so no saved config breaks.
/// A number and an object are unambiguous in JSON, which makes this safe.
impl<'de> Deserialize<'de> for Gaps {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct GapsVisitor;

        impl<'de> Visitor<'de> for GapsVisitor {
            type Value = Gaps;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a gap size (number) or a gaps object")
            }

            fn visit_f64<E>(self, value: f64) -> Result<Gaps, E> {
                Ok(Gaps::uniform(value))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Gaps, E> {
                Ok(Gaps::uniform(value as f64))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Gaps, E> {
                Ok(Gaps::uniform(value as f64))
            }

            fn visit_map<M>(self, mut map: M) -> Result<Gaps, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut gaps = Gaps::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "window" => gaps.window = map.next_value()?,
                        "edgeTop" => gaps.edge_top = map.next_value()?,
                        "edgeBottom" => gaps.edge_bottom = map.next_value()?,
                        "edgeLeft" => gaps.edge_left = map.next_value()?,
                        "edgeRight" => gaps.edge_right = map.next_value()?,
                        "skipTopEdge" => gaps.skip_top_edge = map.next_value()?,
                        "mainScreenOnly" => gaps.main_screen_only = map.next_value()?,
                        // Ignore unknown keys so a newer config never fails to
                        // load on an older build.
                        _ => {
                            let _: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(gaps)
            }
        }

        deserializer.deserialize_any(GapsVisitor)
    }
}

fn default_true() -> bool {
    true
}

/// The default fraction for the [`WindowAction::AlmostMaximize`] size, matching
/// Rectangle's 90% behaviour.
fn default_almost_maximize_fraction() -> f64 {
    0.9
}

/// Clamps a size fraction into `(0, 1]`, falling back to the default when the
/// value is not a usable fraction (e.g. zero, negative, NaN, or above 1).
fn normalize_fraction(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 && value <= 1.0 {
        value
    } else {
        default_almost_maximize_fraction()
    }
}

/// The default step, in the backend's own unit, for one press of an
/// incremental resize or move. Mirrors Rectangle's `sizeOffset` and
/// `widthStepSize`, both of which behave as 30 out of the box.
///
/// The unit is deliberately whatever the backend reports — physical pixels on
/// Windows, points on macOS — for the same reason the rest of the crate never
/// converts between the two. A 30-unit nudge is therefore physically smaller
/// on a high-DPI Windows display than on a Retina Mac, which is the same
/// trade-off Rectangle makes and is easily retuned in the config.
fn default_step() -> f64 {
    30.0
}

/// The smallest fraction of the work area an incremental resize may shrink a
/// window to. Matches Rectangle's `minimumWindowWidth`/`minimumWindowHeight`.
fn default_minimum_fraction() -> f64 {
    0.25
}

/// Upper bound for a resize or move step, so a typo cannot make every press
/// throw the window across the screen.
pub const MAX_STEP: f64 = 1000.0;

/// Clamps a step into `[1, MAX_STEP]`, mapping non-finite or non-positive
/// values to the default. A zero step would make the action a silent no-op.
fn normalize_step(value: f64) -> f64 {
    if !value.is_finite() || value < 1.0 {
        default_step()
    } else {
        value.min(MAX_STEP)
    }
}

/// Clamps a minimum-size fraction into `(0, 1]`, falling back to the default.
fn normalize_minimum_fraction(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 && value <= 1.0 {
        value
    } else {
        default_minimum_fraction()
    }
}

// ---------------------------------------------------------------------------
// Animation
// ---------------------------------------------------------------------------

/// Roughly how long an animated snap takes, end to end, in milliseconds.
///
/// Deliberately towards the languid end. The per-edge springs only pay for
/// themselves if the eye has time to resolve the shape of the motion — the
/// stretch as the leading edge pulls away, the soft overshoot, the trailing
/// edge closing up — and below about 250 ms none of that registers and the
/// animation may as well be a jump. Past about 600 ms the window starts to
/// feel like it is lagging the key rather than answering it.
///
/// This is only the speed dial. How *springy* the motion is lives in the
/// damping ratios in [`crate::animation`] and does not change with it.
fn default_animation_duration_ms() -> u32 {
    450
}

/// Frames per second the animation aims for.
///
/// Above a 60 Hz display's refresh rate, so the motion is smooth on a 60 Hz
/// panel without the driver having to be phase-locked to it, but well short of
/// a rate that would spend meaningful CPU pushing frames at another process's
/// window. Platform backends may cap this further where each frame is
/// expensive; see the frame pump in the app.
fn default_animation_fps() -> u32 {
    90
}

/// Bounds for the animation duration. Below the lower bound the motion is
/// indistinguishable from a teleport but still costs frames; above the upper
/// one the window lags noticeably behind the keypress.
pub const MIN_ANIMATION_DURATION_MS: u32 = 40;
pub const MAX_ANIMATION_DURATION_MS: u32 = 1000;

/// Bounds for the animation frame rate. The floor keeps the motion from
/// looking like a slideshow; the ceiling stops a hand-edited config from
/// hammering another process's window with thousands of position changes a
/// second.
pub const MIN_ANIMATION_FPS: u32 = 15;
pub const MAX_ANIMATION_FPS: u32 = 240;

/// How a window travels to its new frame.
///
/// Only [`AnimationConfig::enabled`] is exposed in the settings UI: it is the
/// choice that matters, and it is what someone who finds motion distracting
/// (or who is running over a remote desktop session) needs to reach. The
/// duration and frame rate are deliberately config-file-only tuning knobs, the
/// same treatment the step sizes get.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AnimationConfig {
    /// Animate window moves instead of applying them in one jump.
    pub enabled: bool,
    /// Roughly how long the movement takes, end to end. Approximate rather
    /// than exact: a spring approaches its target asymptotically, so the value
    /// scales the motion and larger moves run slightly over.
    pub duration_ms: u32,
    /// Frames per second the animation aims to emit.
    pub fps: u32,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            // On by default: the animation is what tells the user their
            // keypress was received and where the window went, and a snap that
            // teleports is the thing this replaces. It is a single checkbox
            // away in the settings window for anyone who wants the instant
            // behaviour back.
            enabled: true,
            duration_ms: default_animation_duration_ms(),
            fps: default_animation_fps(),
        }
    }
}

impl AnimationConfig {
    /// Clamps the tuning knobs, mapping nonsense to the defaults so a
    /// hand-edited config can never stall or spin the frame pump.
    pub fn normalize(&mut self) {
        if self.duration_ms == 0 {
            self.duration_ms = default_animation_duration_ms();
        }
        self.duration_ms = self
            .duration_ms
            .clamp(MIN_ANIMATION_DURATION_MS, MAX_ANIMATION_DURATION_MS);

        if self.fps == 0 {
            self.fps = default_animation_fps();
        }
        self.fps = self.fps.clamp(MIN_ANIMATION_FPS, MAX_ANIMATION_FPS);
    }

    /// The parameters the animator is driven with.
    pub fn params(&self) -> AnimationParams {
        AnimationParams {
            duration_ms: self.duration_ms,
            fps: self.fps,
        }
    }
}

/// Fractions controlling the size-variant actions that resize a window rather
/// than tile it into the grid, plus the step sizes and floor used by the
/// incremental resize and move actions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizeOptions {
    /// [`WindowAction::AlmostMaximize`] width as a fraction of the work area,
    /// in `(0, 1]`.
    pub almost_maximize_width: f64,
    /// [`WindowAction::AlmostMaximize`] height as a fraction of the work area,
    /// in `(0, 1]`.
    pub almost_maximize_height: f64,
    /// How much one `Larger`/`Smaller` press changes a window by, in the
    /// backend's own unit. Rectangle's `sizeOffset`.
    pub size_step: f64,
    /// How much one `LargerWidth`/`SmallerWidth` press changes a window's
    /// width by. Rectangle's `widthStepSize`.
    pub width_step: f64,
    /// How far one `MoveLeft`/`MoveRight`/`MoveUp`/`MoveDown` press slides a
    /// window. Tile-specific; see [`WindowAction::MoveLeft`].
    pub move_step: f64,
    /// Smallest width an incremental resize may leave, as a fraction of the
    /// work area.
    pub minimum_width: f64,
    /// Smallest height an incremental resize may leave, as a fraction of the
    /// work area.
    pub minimum_height: f64,
}

impl Default for SizeOptions {
    fn default() -> Self {
        Self {
            almost_maximize_width: default_almost_maximize_fraction(),
            almost_maximize_height: default_almost_maximize_fraction(),
            size_step: default_step(),
            width_step: default_step(),
            move_step: default_step(),
            minimum_width: default_minimum_fraction(),
            minimum_height: default_minimum_fraction(),
        }
    }
}

/// What a *repeated* press of an already-satisfied action does.
///
/// This mirrors Rectangle's `subsequentExecutionMode`. Rectangle offers six
/// values; Tile ships the two that keep a repeat on the current display:
///
/// * [`SubsequentExecutionMode::CycleSizes`] — Rectangle's `resize`, the
///   default there and here: the window cycles through [`Config::cycle_sizes`].
/// * [`SubsequentExecutionMode::DoNothing`] — Rectangle's `none`: a repeat is
///   a no-op, which is what Tile did before cycling existed.
///
/// Rectangle's remaining values all move the window to another display on a
/// repeat. Tile deliberately does not: moving between displays is bound to
/// [`WindowAction::NextDisplay`] and [`WindowAction::PreviousDisplay`] on
/// their own shortcut instead, which leaves the unmodified arrows free to
/// cycle. That matters because the thirds and two-thirds ship unbound, so
/// cycling is the only way to reach them — a repeat that wandered off to
/// another display would strand every size except the half for anyone with
/// more than one monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubsequentExecutionMode {
    /// Repeating an action walks the window through the configured sizes.
    #[default]
    CycleSizes,
    /// Repeating an action does nothing.
    DoNothing,
}

impl SubsequentExecutionMode {
    /// Stable config identifier.
    pub const fn id(self) -> &'static str {
        match self {
            SubsequentExecutionMode::CycleSizes => "cycle-sizes",
            SubsequentExecutionMode::DoNothing => "do-nothing",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        match id {
            "cycle-sizes" => Some(SubsequentExecutionMode::CycleSizes),
            "do-nothing" => Some(SubsequentExecutionMode::DoNothing),
            _ => None,
        }
    }
}

/// An unknown mode falls back to the default rather than failing the whole
/// load, so a config written by a newer build (say, one that grows a
/// `next-display` mode) still opens on an older one.
impl<'de> Deserialize<'de> for SubsequentExecutionMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(SubsequentExecutionMode::from_id(&raw).unwrap_or_default())
    }
}

/// One step of a size cycle, as a fraction of the work area along the axis the
/// action grows on. Mirrors Rectangle's `CycleSize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CycleSize {
    OneQuarter,
    OneThird,
    OneHalf,
    TwoThirds,
    ThreeQuarters,
}

impl CycleSize {
    /// Every size, smallest first.
    pub const ALL: [CycleSize; 5] = [
        CycleSize::OneQuarter,
        CycleSize::OneThird,
        CycleSize::OneHalf,
        CycleSize::TwoThirds,
        CycleSize::ThreeQuarters,
    ];

    /// The size a cycle always starts from, matching Rectangle's
    /// `CycleSize.firstSize`. It is also the size every cycling action's own
    /// rectangle already has, so the first press lands on it naturally.
    pub const FIRST: CycleSize = CycleSize::OneHalf;

    /// Stable config identifier. These strings are persisted; never rename one.
    pub const fn id(self) -> &'static str {
        match self {
            CycleSize::OneQuarter => "one-quarter",
            CycleSize::OneThird => "one-third",
            CycleSize::OneHalf => "one-half",
            CycleSize::TwoThirds => "two-thirds",
            CycleSize::ThreeQuarters => "three-quarters",
        }
    }

    /// Human-readable label for the settings window.
    pub const fn label(self) -> &'static str {
        match self {
            CycleSize::OneQuarter => "One Quarter",
            CycleSize::OneThird => "One Third",
            CycleSize::OneHalf => "One Half",
            CycleSize::TwoThirds => "Two Thirds",
            CycleSize::ThreeQuarters => "Three Quarters",
        }
    }

    /// The fraction of the work area this size occupies.
    pub const fn fraction(self) -> f64 {
        match self {
            CycleSize::OneQuarter => 1.0 / 4.0,
            CycleSize::OneThird => 1.0 / 3.0,
            CycleSize::OneHalf => 1.0 / 2.0,
            CycleSize::TwoThirds => 2.0 / 3.0,
            CycleSize::ThreeQuarters => 3.0 / 4.0,
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        CycleSize::ALL.into_iter().find(|s| s.id() == id)
    }

    /// Position in the canonical cycle order: start at [`CycleSize::FIRST`],
    /// grow to the largest size, then wrap around to the smallest and grow
    /// back. With every size selected that is ½, ⅔, ¾, ¼, ⅓ — Rectangle's
    /// `CycleSize.sortedSizes`.
    fn cycle_rank(self) -> u8 {
        match self {
            // Ranked so that FIRST leads, larger sizes follow in ascending
            // order, then the smaller ones, also ascending.
            CycleSize::OneHalf => 0,
            CycleSize::TwoThirds => 1,
            CycleSize::ThreeQuarters => 2,
            CycleSize::OneQuarter => 3,
            CycleSize::OneThird => 4,
        }
    }
}

/// An unrecognised entry is dropped rather than failing the load, for the same
/// forward-compatibility reason as [`SubsequentExecutionMode`].
impl<'de> Deserialize<'de> for CycleSize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        CycleSize::from_id(&raw)
            .ok_or_else(|| de::Error::custom(format!("unknown cycle size {raw}")))
    }
}

fn deserialize_cycle_sizes<'de, D>(deserializer: D) -> Result<Vec<CycleSize>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Vec::<String>::deserialize(deserializer)?;
    Ok(raw.iter().filter_map(|s| CycleSize::from_id(s)).collect())
}

/// The sizes a repeated shortcut cycles through by default, matching
/// Rectangle's `CycleSize.defaultSizes` — a half, then two thirds, then a
/// third.
fn default_cycle_sizes() -> Vec<CycleSize> {
    vec![
        CycleSize::OneHalf,
        CycleSize::TwoThirds,
        CycleSize::OneThird,
    ]
}

/// Persisted user settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Hotkey per action. An action absent from the map, or mapped to `None`,
    /// is unbound.
    pub bindings: BTreeMap<WindowAction, Option<Hotkey>>,
    /// Gap model between windows and against the screen edges. Persisted under
    /// the `gap` key; a legacy scalar value still loads.
    #[serde(rename = "gap")]
    pub gaps: Gaps,
    /// Start Tile when the user logs in.
    pub launch_on_login: bool,
    /// Show the tray / menu-bar icon.
    pub show_tray_icon: bool,
    /// [`WindowAction::AlmostMaximize`] width as a fraction of the work area.
    #[serde(default = "default_almost_maximize_fraction")]
    pub almost_maximize_width: f64,
    /// [`WindowAction::AlmostMaximize`] height as a fraction of the work area.
    #[serde(default = "default_almost_maximize_fraction")]
    pub almost_maximize_height: f64,
    /// How much one [`WindowAction::Larger`] or [`WindowAction::Smaller`]
    /// press changes a window by. Rectangle's `sizeOffset`.
    #[serde(default = "default_step")]
    pub size_step: f64,
    /// How much one [`WindowAction::LargerWidth`] or
    /// [`WindowAction::SmallerWidth`] press changes a window's width by.
    /// Rectangle's `widthStepSize`.
    #[serde(default = "default_step")]
    pub width_step: f64,
    /// How far one [`WindowAction::MoveLeft`] and friends slide a window.
    #[serde(default = "default_step")]
    pub move_step: f64,
    /// Smallest width an incremental resize may leave, as a fraction of the
    /// work area. Rectangle's `minimumWindowWidth`.
    #[serde(default = "default_minimum_fraction")]
    pub minimum_window_width: f64,
    /// Smallest height an incremental resize may leave, as a fraction of the
    /// work area. Rectangle's `minimumWindowHeight`.
    #[serde(default = "default_minimum_fraction")]
    pub minimum_window_height: f64,
    /// What a repeated press of an already-satisfied action does.
    #[serde(default)]
    pub subsequent_execution_mode: SubsequentExecutionMode,
    /// The sizes a repeated press cycles through, in cycle order. An empty
    /// list disables cycling as surely as
    /// [`SubsequentExecutionMode::DoNothing`] does.
    #[serde(
        default = "default_cycle_sizes",
        deserialize_with = "deserialize_cycle_sizes"
    )]
    pub cycle_sizes: Vec<CycleSize>,
    /// How a window travels to its new frame. Absent from configs written
    /// before animation existed, hence the serde default.
    #[serde(default)]
    pub animation: AnimationConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bindings: default_bindings(),
            gaps: Gaps::default(),
            launch_on_login: false,
            show_tray_icon: default_true(),
            almost_maximize_width: default_almost_maximize_fraction(),
            almost_maximize_height: default_almost_maximize_fraction(),
            size_step: default_step(),
            width_step: default_step(),
            move_step: default_step(),
            minimum_window_width: default_minimum_fraction(),
            minimum_window_height: default_minimum_fraction(),
            subsequent_execution_mode: SubsequentExecutionMode::default(),
            cycle_sizes: default_cycle_sizes(),
            animation: AnimationConfig::default(),
        }
    }
}

impl Config {
    /// Parses configuration from JSON, filling in defaults for missing fields.
    pub fn from_json(json: &str) -> Result<Self, ConfigError> {
        let mut config: Config = serde_json::from_str(json)?;
        config.normalize();
        Ok(config)
    }

    pub fn to_json(&self) -> Result<String, ConfigError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Drops invalid bindings and clamps out-of-range values so a hand-edited
    /// or older config can never put the app into a broken state.
    pub fn normalize(&mut self) {
        self.bindings.retain(|_, hk| match hk {
            Some(h) => h.is_valid(),
            None => true,
        });
        self.gaps.normalize();
        self.almost_maximize_width = normalize_fraction(self.almost_maximize_width);
        self.almost_maximize_height = normalize_fraction(self.almost_maximize_height);
        self.size_step = normalize_step(self.size_step);
        self.width_step = normalize_step(self.width_step);
        self.move_step = normalize_step(self.move_step);
        self.minimum_window_width = normalize_minimum_fraction(self.minimum_window_width);
        self.minimum_window_height = normalize_minimum_fraction(self.minimum_window_height);
        self.normalize_cycle_sizes();
        self.animation.normalize();
    }

    /// Drops duplicates and puts the cycle into its canonical order, so the
    /// sequence a user sees does not depend on the order the settings window
    /// happened to write the sizes in.
    fn normalize_cycle_sizes(&mut self) {
        self.cycle_sizes.sort_by_key(|s| s.cycle_rank());
        self.cycle_sizes.dedup();
    }

    /// The sizes a repeated press cycles through, in cycle order.
    pub fn cycle_sizes(&self) -> &[CycleSize] {
        &self.cycle_sizes
    }

    /// Whether a repeat should cycle at all: the mode has to allow it and
    /// there has to be at least one size to cycle through.
    pub fn cycles_sizes(&self) -> bool {
        self.subsequent_execution_mode == SubsequentExecutionMode::CycleSizes
            && !self.cycle_sizes.is_empty()
    }

    /// The size-variant fractions, derived from the persisted configuration.
    pub fn size_options(&self) -> SizeOptions {
        SizeOptions {
            almost_maximize_width: self.almost_maximize_width,
            almost_maximize_height: self.almost_maximize_height,
            size_step: self.size_step,
            width_step: self.width_step,
            move_step: self.move_step,
            minimum_width: self.minimum_window_width,
            minimum_height: self.minimum_window_height,
        }
    }

    pub fn binding(&self, action: WindowAction) -> Option<Hotkey> {
        self.bindings.get(&action).copied().flatten()
    }

    /// Binds `hotkey` to `action`, unbinding any other action that already
    /// used it so the configuration can never contain a duplicate.
    pub fn set_binding(&mut self, action: WindowAction, hotkey: Option<Hotkey>) {
        if let Some(hk) = hotkey {
            let clashing: Vec<_> = self
                .bindings
                .iter()
                .filter(|(a, h)| **a != action && **h == Some(hk))
                .map(|(a, _)| *a)
                .collect();
            for a in clashing {
                self.bindings.insert(a, None);
            }
        }
        self.bindings.insert(action, hotkey);
    }

    /// Returns every hotkey bound to more than one action.
    pub fn conflicts(&self) -> Vec<Conflict> {
        let mut by_hotkey: BTreeMap<String, (Hotkey, Vec<WindowAction>)> = BTreeMap::new();
        for (action, hotkey) in &self.bindings {
            if let Some(hk) = hotkey {
                by_hotkey
                    .entry(hk.to_string())
                    .or_insert_with(|| (*hk, Vec::new()))
                    .1
                    .push(*action);
            }
        }
        by_hotkey
            .into_values()
            .filter(|(_, actions)| actions.len() > 1)
            .map(|(hotkey, actions)| Conflict { hotkey, actions })
            .collect()
    }

    /// Every currently bound (hotkey, action) pair, for backend registration.
    pub fn active_bindings(&self) -> Vec<(Hotkey, WindowAction)> {
        self.bindings
            .iter()
            .filter_map(|(a, h)| h.map(|h| (h, *a)))
            .collect()
    }
}

/// Upper bound for the screen-edge gap, to stop a typo shrinking windows away.
pub const MAX_GAP: f64 = 200.0;

/// The modifier carrying the bulk of the default bindings.
///
/// macOS uses `Control+Option`, matching Rectangle's alternate ("Magnet")
/// default set, which is also what Tile has always shipped.
///
/// Windows uses `Win` alone. `Ctrl+Alt` would mirror macOS exactly, but
/// **Windows treats `Ctrl+Alt` as `AltGr`**: on many international layouts
/// `AltGr`+key produces characters such as `@ € { } [ ] \ ~`. Because the
/// Windows backend swallows any keystroke it matches, binding `Ctrl+Alt`+letter
/// would make those characters impossible to type — a German user could not
/// type `@`.
#[cfg(target_os = "macos")]
const BASE_MODIFIERS: Modifiers = Modifiers(Modifiers::CONTROL.0 | Modifiers::ALT.0);
#[cfg(not(target_os = "macos"))]
const BASE_MODIFIERS: Modifiers = Modifiers::META;

/// Default key bindings.
///
/// **Every default sits on the same base modifier**, so the two platforms
/// differ in exactly one thing: what that modifier is. `Control+Option` on
/// macOS, `Win` on Windows. Nothing else varies, so a user with a Mac and a
/// PC learns one set of shortcuts.
///
/// The defaults are the four arrows, plus Shift on Left/Right to throw the
/// window to the adjacent display. `Left` and `Right` place the window and
/// carry the whole size catalogue by cycling; `Up` and `Down` are the
/// "bigger / undo" axis; Shift+Left/Right keep the current slot and walk
/// screens:
///
/// ```text
///   Left   ½ → ⅔ → ⅓ → …   anchored left
///   Right  ½ → ⅔ → ⅓ → …   anchored right
///   Up     maximize
///   Down   restore
/// ```
///
/// **No default sits on a letter**, none needs `Enter` or `Backspace`. The
/// only extra modifier is `Shift` on the horizontal arrows, which throws
/// rather than cycling size. Center, the corners, maximize-height and the
/// centred column are all in the catalogue but ship unbound, because every
/// letter within reach is a left-hand key and the modifier is already a
/// left-hand hold.
///
/// # Why the arrows, and not letters
///
/// Every default is pressed while the base modifier is held, and on a MacBook
/// there is no right `Control`, so `Control+Option` can only be held with the
/// left hand. That makes left-hand letters — the old `A`/`S`/`D` thirds and
/// `Q`/`E` two-thirds — a one-handed contortion. The arrows sit under the
/// right hand, so the two hands divide the work.
///
/// Dropping those letters also retires a Windows problem: the block sat at
/// `Q`/`A` only because Game Bar reserves the keys Rectangle uses, and center
/// two thirds shipped unbound entirely because its natural key was the
/// reserved `W`. None of that constrains an arrow.
///
/// The cost is that a cycling arrow is stateful — landing on a third can take
/// several presses, where a letter was one. The explicitly-sized actions all
/// remain in the catalogue for anyone who wants that determinism back; they
/// are simply not bound by default.
///
/// The vertical halves are unbound for the same reason: `Up` and `Down` are
/// worth more as maximize and restore, and top/bottom halves are a
/// portrait-monitor need rather than a universal one. The centred column keeps
/// its full cycling behaviour — including the backwards step through
/// [`WindowAction::cycle_anchor`] — for anyone who binds it.
///
/// # Why Windows uses `Win+Arrow`
///
/// `Win+Arrow` is Aero Snap — the shell combination people already reach for
/// to tile a window. Tile's hook exists so it can claim those keys; swallowing
/// them replaces Aero Snap with Tile's cycle (half → two thirds → third) on
/// the same four arrows. `Win+Shift+Arrow` (move between monitors) and
/// `Win+Ctrl+Left/Right` (virtual desktops) stay unbound, so those OS
/// shortcuts keep working.
///
/// `Win+Alt+Arrow` is the previous default. It left Aero Snap alone, but it
/// is also Windows 11's snap variants and costs an extra modifier for no
/// extra reach: the defaults are still just the four arrows.
///
/// # Keys Xbox Game Bar reserves
///
/// Game Bar owns eight shortcuts, and Tile cannot win any of them. `Win+Alt+G`
/// is the clearest example: Game Bar's own hotkey is `Win+G` and its handler
/// matches *loosely*, ignoring the extra `Alt`, so the overlay appears even
/// with Tile shut down. The rest are handled by GameDVR through an input path
/// that never reaches the keyboard hook. Users cannot fix this either — Game
/// Bar's settings only *add* shortcuts, they never replace the built-in one.
///
/// The authoritative list is the `VK*` values under
/// `HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\GameDVR`:
///
/// | Action | Shortcut |
/// |---|---|
/// | `ToggleGameBar` | `Win+G` |
/// | `SaveHistoricalVideo` | `Win+Alt+G` |
/// | `ToggleRecording` | `Win+Alt+R` |
/// | `ToggleMicrophoneCapture` | `Win+Alt+M` |
/// | `ToggleBroadcast` | `Win+Alt+B` |
/// | `ToggleCameraCapture` | `Win+Alt+W` |
/// | `ToggleRecordingIndicator` | `Win+Alt+T` |
/// | `TakeScreenshot` | `Win+Alt+PrtScn` |
///
/// So `G`, `R`, `M`, `B`, `W` and `T` are all unusable. No default lands on
/// any of them, and there is a test enforcing the whole set. This used to
/// constrain the layout heavily — it is why the letter block sat at `Q`/`A`
/// and why center two thirds, whose natural key was `W`, shipped unbound. The
/// arrow-and-cycle defaults sidestep the problem entirely: no default sits on
/// a letter, so none of the reserved keys can collide.
///
/// # Why not Rectangle's letters
///
/// Rectangle puts the thirds on `D`/`F`/`G` with `E`/`R`/`T` above, and Tile
/// shipped that briefly. Four of those six keys are reserved by Game Bar, so
/// it moved to `Q`/`A`; that block is now retired in favour of cycling the
/// arrows, which needs no letters at all. Both platforms match either way.
pub fn default_bindings() -> BTreeMap<WindowAction, Option<Hotkey>> {
    let mut map = BTreeMap::new();
    let base = BASE_MODIFIERS;

    // Every default sits on the base modifier, so the two platforms differ
    // only in what that modifier is. The three horizontal arrows carry the
    // whole size catalogue by cycling; maximize mirrors Rectangle's Enter.
    map.insert(
        WindowAction::LeftHalf,
        Some(Hotkey::new(base, KeyCode::Left)),
    );
    map.insert(
        WindowAction::RightHalf,
        Some(Hotkey::new(base, KeyCode::Right)),
    );
    // Up maximizes and Down restores: the vertical pair is the "bigger /
    // undo" axis, while the horizontal pair places the window.
    map.insert(WindowAction::Maximize, Some(Hotkey::new(base, KeyCode::Up)));
    map.insert(
        WindowAction::Restore,
        Some(Hotkey::new(base, KeyCode::Down)),
    );

    // Shift on the same arrows throws to the adjacent display, keeping the
    // current slot. Size cycling stays on the unmodified arrows.
    let throw = base.union(Modifiers::SHIFT);
    map.insert(
        WindowAction::PreviousDisplay,
        Some(Hotkey::new(throw, KeyCode::Left)),
    );
    map.insert(
        WindowAction::NextDisplay,
        Some(Hotkey::new(throw, KeyCode::Right)),
    );

    map
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actions that ship with a default binding.
    ///
    /// Four arrows for tiling, plus Shift+Left/Right for display throws.
    /// Every other action — center, the corners, maximize-height, the centred
    /// column and the explicitly-sized thirds and two-thirds — ships unbound:
    /// every letter within reach is a left-hand key and the modifier is
    /// already a left-hand hold.
    const CORE_BOUND: [WindowAction; 6] = [
        WindowAction::LeftHalf,
        WindowAction::RightHalf,
        WindowAction::Maximize,
        WindowAction::Restore,
        WindowAction::PreviousDisplay,
        WindowAction::NextDisplay,
    ];

    /// Both horizontal arrows must cycle, because that is the only way the
    /// thirds and two-thirds are reachable now that they ship unbound. A
    /// future edit binding one to an explicitly-sized action would silently
    /// strand every size except the half.
    #[test]
    fn both_horizontal_arrows_cycle() {
        let config = Config::default();
        for action in [WindowAction::LeftHalf, WindowAction::RightHalf] {
            assert!(
                action.cycles(),
                "{action} must cycle to reach the sizes that ship unbound"
            );
            assert!(
                config.binding(action).is_some(),
                "{action} must keep its arrow binding"
            );
        }

        // The centred column is still cycleable, just no longer bound: Up and
        // Down are maximize and restore. Keep the machinery intact so a user
        // binding CenterHalf gets the full cycle, backwards step included.
        assert!(WindowAction::CenterHalf.cycles());
        assert_eq!(
            WindowAction::CenterHalfBack.cycle_anchor(),
            WindowAction::CenterHalf
        );
        assert!(WindowAction::CenterHalfBack.cycles_backwards());
        assert_eq!(config.binding(WindowAction::CenterHalf), None);
        assert_eq!(config.binding(WindowAction::CenterHalfBack), None);

        // The sizes these arrows stand in for must stay in the catalogue, so
        // the tray menu and custom bindings can still reach them directly.
        for action in [
            WindowAction::FirstThird,
            WindowAction::CenterThird,
            WindowAction::LastThird,
            WindowAction::FirstTwoThirds,
            WindowAction::CenterTwoThirds,
            WindowAction::LastTwoThirds,
        ] {
            assert_eq!(
                config.binding(action),
                None,
                "{action} is reached by cycling an arrow and should ship unbound"
            );
        }
    }

    /// No default may sit on a letter. The whole point of the arrow layout is
    /// that the modifier hand and the action hand are different hands.
    #[test]
    fn no_default_binding_uses_a_letter() {
        let config = Config::default();
        for (hotkey, action) in config.active_bindings() {
            assert!(
                !('A'..='Z').any(|c| hotkey.key.label() == c.to_string()),
                "{action} is bound to the letter {} ({hotkey})",
                hotkey.key.label()
            );
        }
    }

    #[test]
    fn defaults_use_spatially_mnemonic_keys() {
        // These keys are load-bearing, not arbitrary. The horizontal pair
        // places the window and cycles its width; the vertical pair is the
        // "bigger / undo" axis:
        //
        //   Left / Right   place and resize
        //   Up / Down      maximize and restore
        //
        // Changing one silently breaks the mnemonic, so they are pinned here.
        let config = Config::default();
        let expected = [
            (WindowAction::LeftHalf, KeyCode::Left),
            (WindowAction::RightHalf, KeyCode::Right),
            (WindowAction::Maximize, KeyCode::Up),
            (WindowAction::Restore, KeyCode::Down),
        ];
        for (action, key) in expected {
            let hotkey = config.binding(action).expect("action must be bound");
            assert_eq!(hotkey.key, key, "{action} lost its mnemonic key");
            assert_eq!(
                hotkey.modifiers, BASE_MODIFIERS,
                "{action} should use the platform base modifier"
            );
        }

        let throw = BASE_MODIFIERS.union(Modifiers::SHIFT);
        for (action, key) in [
            (WindowAction::PreviousDisplay, KeyCode::Left),
            (WindowAction::NextDisplay, KeyCode::Right),
        ] {
            let hotkey = config.binding(action).expect("display throw must be bound");
            assert_eq!(hotkey.key, key, "{action} lost its throw key");
            assert_eq!(
                hotkey.modifiers, throw,
                "{action} should be the base modifier plus Shift"
            );
        }

        // Nothing else ships bound: the vertical halves, center, the corners
        // and the centred column are all catalogue-only.
        for action in [
            WindowAction::TopHalf,
            WindowAction::BottomHalf,
            WindowAction::Center,
            WindowAction::CenterHalf,
            WindowAction::CenterHalfBack,
            WindowAction::TopLeft,
            WindowAction::TopRight,
            WindowAction::BottomLeft,
            WindowAction::BottomRight,
        ] {
            assert_eq!(
                config.binding(action),
                None,
                "{action} must stay unbound under the arrow-only defaults"
            );
        }
    }

    /// Every default must be identical across platforms apart from the base
    /// modifier, so someone with a Mac and a PC learns one set of shortcuts.
    /// Hard-coded rather than derived, so a platform-specific edit to
    /// `default_bindings` fails here instead of drifting silently.
    #[test]
    fn defaults_differ_only_by_the_base_modifier() {
        let config = Config::default();
        let expected = [
            (WindowAction::LeftHalf, KeyCode::Left),
            (WindowAction::RightHalf, KeyCode::Right),
            (WindowAction::Maximize, KeyCode::Up),
            (WindowAction::Restore, KeyCode::Down),
        ];
        for (action, key) in expected {
            let hotkey = config.binding(action).expect("action must be bound");
            assert_eq!(
                hotkey.key, key,
                "{action} must use the same key on every platform"
            );
            assert_eq!(
                hotkey.modifiers, BASE_MODIFIERS,
                "{action} should sit on the base modifier"
            );
        }

        let throw = BASE_MODIFIERS.union(Modifiers::SHIFT);
        for (action, key) in [
            (WindowAction::PreviousDisplay, KeyCode::Left),
            (WindowAction::NextDisplay, KeyCode::Right),
        ] {
            let hotkey = config.binding(action).expect("display throw must be bound");
            assert_eq!(hotkey.key, key);
            assert_eq!(hotkey.modifiers, throw);
        }

        for (hotkey, action) in config.active_bindings() {
            let expected = if action.moves_display() {
                throw
            } else {
                BASE_MODIFIERS
            };
            assert_eq!(
                hotkey.modifiers, expected,
                "{action} has unexpected modifiers ({hotkey})"
            );
        }
    }

    /// Windows defaults are `Win` plus an arrow, not `Win+Alt`. Pin that so a
    /// well-meaning "leave Aero Snap alone" edit cannot silently restore the
    /// extra modifier.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn windows_defaults_use_win_without_alt() {
        assert_eq!(BASE_MODIFIERS, Modifiers::META);
        let config = Config::default();
        for (hotkey, action) in config.active_bindings() {
            assert!(
                hotkey.modifiers.contains(Modifiers::META),
                "{action} is {hotkey}, expected Win"
            );
            assert!(
                !hotkey.modifiers.contains(Modifiers::ALT),
                "{action} is {hotkey}, Win+Alt is no longer the default"
            );
            if action.moves_display() {
                assert_eq!(
                    hotkey.modifiers,
                    Modifiers::META | Modifiers::SHIFT,
                    "{action} is {hotkey}, expected Win+Shift"
                );
            } else {
                assert_eq!(
                    hotkey.modifiers,
                    Modifiers::META,
                    "{action} is {hotkey}, expected Win alone"
                );
            }
        }
    }

    /// Windows treats `Ctrl+Alt` as `AltGr`, which many international layouts
    /// use to type `@ € { } [ ] \ ~`. Since the Windows backend swallows the
    /// keystrokes it matches, a `Ctrl+Alt` default would make those characters
    /// untypeable. Guard against anyone "simplifying" the modifiers to match
    /// macOS exactly.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn windows_defaults_never_use_ctrl_alt() {
        let config = Config::default();
        for (hotkey, action) in config.active_bindings() {
            let ctrl_alt = Modifiers::CONTROL | Modifiers::ALT;
            assert!(
                !hotkey.modifiers.contains(ctrl_alt),
                "{action} uses Ctrl+Alt ({hotkey}), which collides with AltGr"
            );
        }
    }

    /// Xbox Game Bar owns eight shortcuts that Tile cannot win, listed against
    /// `default_bindings`. `Win+Alt+G` fires even with Tile shut down, because
    /// Game Bar's `Win+G` matches loosely; the others are handled by GameDVR
    /// through an input path the keyboard hook never sees. Users cannot
    /// disable them — Game Bar's settings only add shortcuts.
    ///
    /// The defaults were chosen to avoid all of these. This test is the guard,
    /// and it is deliberately exhaustive: an earlier version listed only `G`,
    /// `R`, `M` and `B`, which let a `W` binding ship and collide with Game
    /// Bar's broadcast camera toggle.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn defaults_avoid_the_unwinnable_game_bar_shortcuts() {
        // ToggleGameBar / SaveHistoricalVideo, ToggleRecording,
        // ToggleMicrophoneCapture, ToggleBroadcast, ToggleCameraCapture,
        // ToggleRecordingIndicator. (`Win+Alt+PrtScn` is reserved too, but
        // Tile has no PrintScreen key code, so it cannot be bound at all.)
        const RESERVED: [KeyCode; 6] = [
            KeyCode::G,
            KeyCode::R,
            KeyCode::M,
            KeyCode::B,
            KeyCode::W,
            KeyCode::T,
        ];

        let config = Config::default();
        for (hotkey, action) in config.active_bindings() {
            if !hotkey.modifiers.contains(Modifiers::META) {
                continue;
            }
            assert!(
                !RESERVED.contains(&hotkey.key),
                "{action} is bound to {hotkey}, which Game Bar reserves and Tile cannot override"
            );
        }
    }

    #[test]
    fn defaults_bind_the_core_actions_without_conflicts() {
        let config = Config::default();
        for action in CORE_BOUND {
            assert!(
                config.binding(action).is_some(),
                "{action} has no default binding"
            );
        }
        // Every other action ships unbound.
        for action in WindowAction::ALL {
            if !CORE_BOUND.contains(&action) {
                assert!(
                    config.binding(action).is_none(),
                    "{action} unexpectedly has a default binding"
                );
            }
        }
        assert_eq!(
            config.conflicts(),
            vec![],
            "default bindings must not clash"
        );
    }

    #[test]
    fn defaults_round_trip_through_json() {
        let config = Config::default();
        let parsed = Config::from_json(&config.to_json().unwrap()).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let config = Config::from_json("{}").unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn setting_a_binding_unbinds_the_previous_owner() {
        let mut config = Config::default();
        let hk = config.binding(WindowAction::LeftHalf).unwrap();
        config.set_binding(WindowAction::Center, Some(hk));
        assert_eq!(config.binding(WindowAction::LeftHalf), None);
        assert_eq!(config.binding(WindowAction::Center), Some(hk));
        assert_eq!(config.conflicts(), vec![]);
    }

    #[test]
    fn conflicts_are_detected() {
        let mut config = Config::default();
        let hk = config.binding(WindowAction::LeftHalf).unwrap();
        // Bypass set_binding to construct a config like a hand-edited file.
        config.bindings.insert(WindowAction::Center, Some(hk));
        let conflicts = config.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].actions.len(), 2);
    }

    #[test]
    fn normalize_clamps_bad_gaps() {
        let mut config = Config {
            gaps: Gaps::uniform(-5.0),
            ..Default::default()
        };
        config.normalize();
        assert_eq!(config.gaps, Gaps::default());

        let mut config = Config {
            gaps: Gaps::uniform(10_000.0),
            ..Default::default()
        };
        config.normalize();
        assert_eq!(config.gaps, Gaps::uniform(MAX_GAP));

        let config = Config::from_json(r#"{"gap": -1}"#).unwrap();
        assert_eq!(config.gaps, Gaps::default());
    }

    #[test]
    fn legacy_scalar_gap_migrates_to_uniform_gaps() {
        // A config saved by an older build stores a plain number.
        let config = Config::from_json(r#"{"gap": 24}"#).unwrap();
        assert_eq!(config.gaps, Gaps::uniform(24.0));
        assert_eq!(config.gaps.window, 24.0);
        assert_eq!(config.gaps.edge_left, 24.0);
        assert!(!config.gaps.skip_top_edge);

        // A fractional legacy scalar still loads.
        let config = Config::from_json(r#"{"gap": 12.5}"#).unwrap();
        assert_eq!(config.gaps, Gaps::uniform(12.5));
    }

    #[test]
    fn new_object_gaps_load_and_partial_objects_fall_back_to_defaults() {
        let json = r#"{"gap": {
            "window": 8,
            "edgeTop": 0,
            "edgeBottom": 10,
            "edgeLeft": 12,
            "edgeRight": 12,
            "skipTopEdge": true,
            "mainScreenOnly": true
        }}"#;
        let config = Config::from_json(json).unwrap();
        assert_eq!(config.gaps.window, 8.0);
        assert_eq!(config.gaps.edge_top, 0.0);
        assert_eq!(config.gaps.edge_bottom, 10.0);
        assert_eq!(config.gaps.edge_left, 12.0);
        assert!(config.gaps.skip_top_edge);
        assert!(config.gaps.main_screen_only);

        // Missing object fields fall back to defaults (zero / false).
        let config = Config::from_json(r#"{"gap": {"window": 6}}"#).unwrap();
        assert_eq!(config.gaps.window, 6.0);
        assert_eq!(config.gaps.edge_left, 0.0);
        assert!(!config.gaps.main_screen_only);
    }

    #[test]
    fn gaps_object_round_trips_through_json() {
        let config = Config {
            gaps: Gaps {
                window: 8.0,
                edge_top: 0.0,
                edge_bottom: 10.0,
                edge_left: 12.0,
                edge_right: 12.0,
                skip_top_edge: true,
                main_screen_only: false,
            },
            ..Default::default()
        };
        let parsed = Config::from_json(&config.to_json().unwrap()).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn cycling_defaults_match_rectangle() {
        let config = Config::default();
        assert_eq!(
            config.subsequent_execution_mode,
            SubsequentExecutionMode::CycleSizes
        );
        assert_eq!(
            config.cycle_sizes(),
            [
                CycleSize::OneHalf,
                CycleSize::TwoThirds,
                CycleSize::OneThird
            ]
        );
        assert!(config.cycles_sizes());
    }

    #[test]
    fn a_config_predating_cycling_still_loads() {
        // Exactly what an older build would have written: no cycling keys at
        // all, and the legacy scalar gap for good measure.
        let json = r#"{
            "bindings": {},
            "gap": 8,
            "launchOnLogin": true,
            "showTrayIcon": false
        }"#;
        let config = Config::from_json(json).unwrap();
        assert_eq!(config.gaps, Gaps::uniform(8.0));
        assert!(config.launch_on_login);
        assert!(!config.show_tray_icon);
        // Cycling arrives switched on, with Rectangle's defaults.
        assert_eq!(
            config.subsequent_execution_mode,
            SubsequentExecutionMode::CycleSizes
        );
        assert_eq!(config.cycle_sizes(), Config::default().cycle_sizes());
        // The incremental resize and move steps arrive at their defaults too.
        assert_eq!(config.size_step, 30.0);
        assert_eq!(config.width_step, 30.0);
        assert_eq!(config.move_step, 30.0);
        assert_eq!(config.minimum_window_width, 0.25);
        assert_eq!(config.minimum_window_height, 0.25);
    }

    #[test]
    fn animation_defaults_are_on_and_round_trip_through_json() {
        let config = Config::default();
        assert!(config.animation.enabled);
        assert_eq!(config.animation.duration_ms, 450);
        assert_eq!(config.animation.fps, 90);

        let json = config.to_json().unwrap();
        assert!(json.contains(r#""durationMs": 450"#));
        assert_eq!(Config::from_json(&json).unwrap(), config);
    }

    #[test]
    fn a_config_predating_animation_still_loads() {
        // A file written by a build that had no concept of animation at all.
        // It must load, and pick up the current defaults rather than an
        // all-zero struct that would divide by zero in the frame pump.
        let json = r#"{
            "bindings": {},
            "gap": 8,
            "launchOnLogin": true
        }"#;
        let config = Config::from_json(json).unwrap();
        assert_eq!(config.animation, AnimationConfig::default());
    }

    #[test]
    fn a_partial_animation_object_keeps_the_other_defaults() {
        // Someone switching the animation off by hand should not have to
        // restate the tuning knobs.
        let config = Config::from_json(r#"{"animation": {"enabled": false}}"#).unwrap();
        assert!(!config.animation.enabled);
        assert_eq!(config.animation.duration_ms, 450);
        assert_eq!(config.animation.fps, 90);
    }

    #[test]
    fn normalize_clamps_animation_tuning() {
        let mut config = Config {
            animation: AnimationConfig {
                enabled: true,
                duration_ms: 99_999,
                fps: 5,
            },
            ..Default::default()
        };
        config.normalize();
        assert_eq!(config.animation.duration_ms, MAX_ANIMATION_DURATION_MS);
        assert_eq!(config.animation.fps, MIN_ANIMATION_FPS);

        // Zero is the dangerous one: a zero duration divides by zero when the
        // animator scales time, and a zero fps does the same for the frame
        // interval. Both fall back to the default rather than being clamped to
        // the floor, since zero reads as "unset" more than "as fast as
        // possible".
        let mut zeroed = Config {
            animation: AnimationConfig {
                enabled: true,
                duration_ms: 0,
                fps: 0,
            },
            ..Default::default()
        };
        zeroed.normalize();
        assert_eq!(zeroed.animation.duration_ms, 450);
        assert_eq!(zeroed.animation.fps, 90);

        let mut tiny = Config {
            animation: AnimationConfig {
                enabled: true,
                duration_ms: 1,
                fps: 1000,
            },
            ..Default::default()
        };
        tiny.normalize();
        assert_eq!(tiny.animation.duration_ms, MIN_ANIMATION_DURATION_MS);
        assert_eq!(tiny.animation.fps, MAX_ANIMATION_FPS);
    }

    #[test]
    fn animation_params_follow_the_config() {
        let config = Config {
            animation: AnimationConfig {
                enabled: true,
                duration_ms: 220,
                fps: 60,
            },
            ..Default::default()
        };
        let params = config.animation.params();
        assert_eq!(params.duration_ms, 220);
        assert_eq!(params.fps, 60);
    }

    #[test]
    fn cycling_settings_round_trip_through_json() {
        let config = Config {
            subsequent_execution_mode: SubsequentExecutionMode::DoNothing,
            cycle_sizes: vec![CycleSize::OneHalf, CycleSize::ThreeQuarters],
            ..Default::default()
        };
        let json = config.to_json().unwrap();
        assert!(json.contains(r#""subsequentExecutionMode": "do-nothing""#));
        assert!(json.contains(r#""three-quarters""#));
        assert_eq!(Config::from_json(&json).unwrap(), config);
    }

    #[test]
    fn unknown_cycling_values_fall_back_instead_of_failing_the_load() {
        // A config written by a future build that grew a "cycle-monitor" mode
        // and a size this build does not know about.
        let json = r#"{
            "subsequentExecutionMode": "cycle-monitor",
            "cycleSizes": ["one-half", "one-fifth", "two-thirds"]
        }"#;
        let config = Config::from_json(json).unwrap();
        assert_eq!(
            config.subsequent_execution_mode,
            SubsequentExecutionMode::default()
        );
        assert_eq!(
            config.cycle_sizes(),
            [CycleSize::OneHalf, CycleSize::TwoThirds]
        );
    }

    #[test]
    fn step_settings_round_trip_and_normalize() {
        let config = Config {
            size_step: 45.0,
            width_step: 90.0,
            move_step: 12.0,
            minimum_window_width: 0.1,
            minimum_window_height: 0.4,
            ..Default::default()
        };
        let json = config.to_json().unwrap();
        assert!(json.contains(r#""sizeStep": 45.0"#));
        assert_eq!(Config::from_json(&json).unwrap(), config);

        // Nonsense values fall back to the defaults rather than making an
        // action a silent no-op or shrinking a window away.
        let json = r#"{
            "sizeStep": 0,
            "widthStep": -5,
            "moveStep": 100000,
            "minimumWindowWidth": 2,
            "minimumWindowHeight": -1
        }"#;
        let config = Config::from_json(json).unwrap();
        assert_eq!(config.size_step, 30.0);
        assert_eq!(config.width_step, 30.0);
        assert_eq!(config.move_step, MAX_STEP);
        assert_eq!(config.minimum_window_width, 0.25);
        assert_eq!(config.minimum_window_height, 0.25);
    }

    #[test]
    fn size_options_carry_the_persisted_steps() {
        let config = Config {
            size_step: 15.0,
            move_step: 25.0,
            ..Default::default()
        };
        let options = config.size_options();
        assert_eq!(options.size_step, 15.0);
        assert_eq!(options.move_step, 25.0);
        assert_eq!(options.minimum_width, 0.25);
    }

    #[test]
    fn every_subsequent_execution_mode_round_trips() {
        for mode in [
            SubsequentExecutionMode::CycleSizes,
            SubsequentExecutionMode::DoNothing,
        ] {
            let config = Config {
                subsequent_execution_mode: mode,
                ..Default::default()
            };
            let parsed = Config::from_json(&config.to_json().unwrap()).unwrap();
            assert_eq!(parsed.subsequent_execution_mode, mode, "{}", mode.id());
        }
    }

    #[test]
    fn cycle_sizes_are_deduplicated_and_ordered_canonically() {
        let mut config = Config {
            cycle_sizes: vec![
                CycleSize::OneThird,
                CycleSize::ThreeQuarters,
                CycleSize::OneHalf,
                CycleSize::OneThird,
                CycleSize::OneQuarter,
                CycleSize::TwoThirds,
            ],
            ..Default::default()
        };
        config.normalize();
        // Rectangle's order: start at a half, grow, then wrap to the smallest.
        assert_eq!(
            config.cycle_sizes(),
            [
                CycleSize::OneHalf,
                CycleSize::TwoThirds,
                CycleSize::ThreeQuarters,
                CycleSize::OneQuarter,
                CycleSize::OneThird,
            ]
        );
    }

    #[test]
    fn an_empty_size_list_disables_cycling() {
        let config = Config::from_json(r#"{"cycleSizes": []}"#).unwrap();
        assert!(config.cycle_sizes().is_empty());
        assert!(!config.cycles_sizes());
    }

    #[test]
    fn cycle_size_ids_are_stable_and_unique() {
        let mut ids: Vec<&str> = CycleSize::ALL.iter().map(|s| s.id()).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count);
        // These strings are persisted config keys; pin them.
        assert_eq!(CycleSize::OneHalf.id(), "one-half");
        assert_eq!(CycleSize::TwoThirds.id(), "two-thirds");
        assert_eq!(CycleSize::ThreeQuarters.id(), "three-quarters");
        assert_eq!(CycleSize::OneThird.id(), "one-third");
        assert_eq!(CycleSize::OneQuarter.id(), "one-quarter");
    }

    #[test]
    fn active_bindings_skips_unbound_actions() {
        let mut config = Config::default();
        let before = config.active_bindings().len();
        config.set_binding(WindowAction::Maximize, None);
        let active = config.active_bindings();
        assert_eq!(active.len(), before - 1);
        assert!(!active.iter().any(|(_, a)| *a == WindowAction::Maximize));
    }
}
