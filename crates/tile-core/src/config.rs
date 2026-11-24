//! User configuration: group modifiers, per-action keys and behaviour,
//! persisted as JSON.

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::str::FromStr;

use crate::action::{ShortcutGroup, WindowAction};
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
/// A legacy scalar config (`"gap": 24`) still loads — see the custom
/// [`Deserialize`] impl below.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gaps {
    /// Space between two adjacent tiled windows. Each shares half of it.
    pub window: f64,
    pub edge_top: f64,
    pub edge_bottom: f64,
    pub edge_left: f64,
    pub edge_right: f64,
    /// Drop the top screen-edge gap entirely.
    pub skip_top_edge: bool,
    /// Apply screen-edge gaps only on the primary display. The window gap
    /// always applies.
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

/// Accepts either a legacy scalar (`"gap": 24`) or the object form
/// (`"gap": { "window": 8, "edgeTop": 0, ... }`), so no saved config breaks.
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

// ---------------------------------------------------------------------------
// Animation
// ---------------------------------------------------------------------------

/// How long an animated snap takes by default, end to end, in milliseconds.
fn default_animation_duration_ms() -> u32 {
    crate::animation::PLATFORM_DEFAULT_DURATION_MS
}

/// Bounds for the animation duration.
pub const MIN_ANIMATION_DURATION_MS: u32 = 40;
pub const MAX_ANIMATION_DURATION_MS: u32 = 1000;

/// Fallback pacing rate when the OS cannot report a display's refresh rate.
pub const DEFAULT_ANIMATION_FPS: u32 = 60;

/// Safety bounds for OS-reported display refresh rates.
pub const MIN_ANIMATION_FPS: u32 = 15;
pub const MAX_ANIMATION_FPS: u32 = 240;

/// How a window travels to its new frame.
///
/// Frame pacing follows the target display's refresh rate automatically rather
/// than exposing an fps knob in Settings (legacy `"fps"` entries in older
/// configs are ignored by serde).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AnimationConfig {
    /// Animate window moves instead of applying them in one jump.
    pub enabled: bool,
    /// How long the movement takes, end to end, in milliseconds.
    pub duration_ms: u32,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            duration_ms: default_animation_duration_ms(),
        }
    }
}

impl AnimationConfig {
    /// Clamps the duration, mapping zero to the platform default.
    pub fn normalize(&mut self) {
        if self.duration_ms == 0 {
            self.duration_ms = default_animation_duration_ms();
        }
        self.duration_ms = self
            .duration_ms
            .clamp(MIN_ANIMATION_DURATION_MS, MAX_ANIMATION_DURATION_MS);
    }

    /// The parameters the animator is driven with at `refresh_rate_hz`
    /// (falling back to [`DEFAULT_ANIMATION_FPS`] when unknown).
    pub fn params(&self, refresh_rate_hz: Option<u32>) -> AnimationParams {
        let fps = refresh_rate_hz
            .filter(|&hz| hz > 0)
            .unwrap_or(DEFAULT_ANIMATION_FPS)
            .clamp(MIN_ANIMATION_FPS, MAX_ANIMATION_FPS);
        AnimationParams {
            duration_ms: self.duration_ms,
            fps,
        }
    }
}

/// What a *repeated* press of an already-satisfied placement action does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubsequentExecutionMode {
    /// Repeating an action walks the window through [`Config::cycle_sizes`].
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
/// placement grows on.
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

    /// The size a cycle always starts from.
    pub const FIRST: CycleSize = CycleSize::OneHalf;

    /// Stable config identifier.
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

    /// Canonical cycle order: start at ½, grow to largest, wrap to smallest
    /// and grow back (½, ⅔, ¾, ¼, ⅓).
    fn cycle_rank(self) -> u8 {
        match self {
            CycleSize::OneHalf => 0,
            CycleSize::TwoThirds => 1,
            CycleSize::ThreeQuarters => 2,
            CycleSize::OneQuarter => 3,
            CycleSize::OneThird => 4,
        }
    }
}

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

fn default_cycle_sizes() -> Vec<CycleSize> {
    vec![
        CycleSize::OneHalf,
        CycleSize::TwoThirds,
        CycleSize::OneThird,
    ]
}

fn default_placement_modifiers() -> Modifiers {
    BASE_MODIFIERS
}

fn default_display_modifiers() -> Modifiers {
    DISPLAY_MODIFIERS
}

/// Persisted user settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Base modifier held for window-placement actions (`Left`, `Right`,
    /// `Maximize`, `Restore`, `Center Column`) and corner composition.
    #[serde(default = "default_placement_modifiers")]
    pub placement_modifiers: Modifiers,
    /// Display modifier held for moving windows across displays.
    #[serde(default = "default_display_modifiers")]
    pub display_modifiers: Modifiers,
    /// Key assigned to each action, combined with its group's modifier.
    /// Absent or `None` means unbound.
    #[serde(default = "default_keys")]
    pub keys: BTreeMap<WindowAction, Option<KeyCode>>,
    /// Gap model between windows and against the screen edges. Persisted under
    /// the `gap` key; a legacy scalar value still loads.
    #[serde(rename = "gap")]
    pub gaps: Gaps,
    /// Start Tile when the user logs in.
    pub launch_on_login: bool,
    /// What a repeated press of an already-satisfied action does.
    #[serde(default)]
    pub subsequent_execution_mode: SubsequentExecutionMode,
    /// The sizes a repeated press cycles through, in cycle order.
    #[serde(
        default = "default_cycle_sizes",
        deserialize_with = "deserialize_cycle_sizes"
    )]
    pub cycle_sizes: Vec<CycleSize>,
    /// Whether the one-time first-run orientation has already been shown.
    #[serde(default)]
    pub orientation_shown: bool,
    /// How a window travels to its new frame.
    #[serde(default)]
    pub animation: AnimationConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            placement_modifiers: BASE_MODIFIERS,
            display_modifiers: DISPLAY_MODIFIERS,
            keys: default_keys(),
            gaps: Gaps::default(),
            launch_on_login: true,
            subsequent_execution_mode: SubsequentExecutionMode::default(),
            cycle_sizes: default_cycle_sizes(),
            orientation_shown: false,
            animation: AnimationConfig::default(),
        }
    }
}

/// The config file format this build reads and writes.
///
/// Bumped to 2 when freeform per-action `bindings` were replaced by the
/// two-modifier + per-action `keys` layout. Legacy v1 files migrate cleanly
/// on load without triggering recovery.
pub const CONFIG_SCHEMA_VERSION: u32 = 2;

/// The version assumed for a config written before `schemaVersion` existed.
const LEGACY_SCHEMA_VERSION: u32 = 1;

const SCHEMA_VERSION_KEY: &str = "schemaVersion";
const BINDINGS_KEY: &str = "bindings";
const KEYS_KEY: &str = "keys";
const PLACEMENT_MODIFIERS_KEY: &str = "placementModifiers";
const DISPLAY_MODIFIERS_KEY: &str = "displayModifiers";

/// The result of [`Config::from_json_lenient`]: the best config that could be
/// recovered from a file, and what had to be given up to get it.
#[derive(Debug, Clone, PartialEq)]
pub struct LenientConfig {
    pub config: Config,
    /// The `schemaVersion` the file declared, or the legacy version when it
    /// declared none.
    pub schema_version: u32,
    /// JSON keys that could not be read and fell back to their defaults.
    pub reset_fields: Vec<String>,
    /// Benign notes produced while migrating an older config (e.g. retired
    /// actions dropped or colliding legacy bindings resolved), logged by the
    /// caller without triggering recovery.
    pub migration_notes: Vec<String>,
}

impl LenientConfig {
    /// Whether the file came from a newer build than this one.
    pub fn is_newer_schema(&self) -> bool {
        self.schema_version > CONFIG_SCHEMA_VERSION
    }
}

/// Serializes a [`Config`] with the schema version leading the object.
#[derive(Serialize)]
struct VersionedConfig<'a> {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    #[serde(flatten)]
    config: &'a Config,
}

/// Whether `value` alone deserializes as the `key` field of a [`Config`].
fn field_parses(key: &str, value: &serde_json::Value) -> bool {
    let mut single = serde_json::Map::new();
    single.insert(key.to_owned(), value.clone());
    serde_json::from_value::<Config>(serde_json::Value::Object(single)).is_ok()
}

impl Config {
    /// Parses configuration from JSON, migrating legacy v1 `bindings` and
    /// filling in defaults for missing fields.
    pub fn from_json(json: &str) -> Result<Self, ConfigError> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        let serde_json::Value::Object(mut map) = value else {
            return Err(ConfigError::Invalid(
                "configuration is not a JSON object".into(),
            ));
        };
        if let Some(legacy_bindings) = take_legacy_bindings(&mut map) {
            let mut strict_errors = Vec::new();
            let mut notes = Vec::new();
            let migrated =
                migrate_legacy_bindings(&legacy_bindings, &mut strict_errors, &mut notes)?;
            if !strict_errors.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "invalid legacy bindings: {}",
                    strict_errors.join(", ")
                )));
            }
            apply_migrated_to_map(&mut map, migrated);
        }
        let mut config: Config = serde_json::from_value(serde_json::Value::Object(map))?;
        config.normalize();
        Ok(config)
    }

    /// Parses configuration from JSON one field at a time, so a single
    /// wrong-typed value resets only that value instead of the whole file.
    ///
    /// Legacy v1 `bindings` are migrated silently into `placementModifiers`,
    /// `displayModifiers`, and `keys`, dropping retired actions without
    /// flagging the file as corrupt.
    pub fn from_json_lenient(json: &str) -> Result<LenientConfig, ConfigError> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        let serde_json::Value::Object(mut map) = value else {
            return Err(ConfigError::Invalid(
                "configuration is not a JSON object".into(),
            ));
        };
        let mut reset_fields = Vec::new();
        let mut migration_notes = Vec::new();

        let schema_version = match map.remove(SCHEMA_VERSION_KEY) {
            None => LEGACY_SCHEMA_VERSION,
            Some(raw) => match raw.as_u64().and_then(|v| u32::try_from(v).ok()) {
                Some(version) => version,
                None => {
                    reset_fields.push(SCHEMA_VERSION_KEY.to_owned());
                    LEGACY_SCHEMA_VERSION
                }
            },
        };

        if let Some(legacy_bindings) = take_legacy_bindings(&mut map) {
            match migrate_legacy_bindings(&legacy_bindings, &mut reset_fields, &mut migration_notes)
            {
                Ok(migrated) => apply_migrated_to_map(&mut map, migrated),
                Err(_) => {
                    reset_fields.push(BINDINGS_KEY.to_owned());
                }
            }
        }

        let keys: Vec<String> = map.keys().cloned().collect();
        for key in keys {
            if field_parses(&key, &map[&key]) {
                continue;
            }
            if key == KEYS_KEY {
                if let Some(serde_json::Value::Object(entries)) = map.get_mut(&key) {
                    entries.retain(|action, key_val| {
                        let mut entry = serde_json::Map::new();
                        entry.insert(action.clone(), key_val.clone());
                        let ok = field_parses(KEYS_KEY, &serde_json::Value::Object(entry));
                        if !ok {
                            reset_fields.push(format!("{KEYS_KEY}.{action}"));
                        }
                        ok
                    });
                    continue;
                }
            }
            map.remove(&key);
            reset_fields.push(key);
        }

        let mut config: Config = serde_json::from_value(serde_json::Value::Object(map))?;
        config.normalize();
        Ok(LenientConfig {
            config,
            schema_version,
            reset_fields,
            migration_notes,
        })
    }

    /// Serializes the config, always stamped with [`CONFIG_SCHEMA_VERSION`].
    pub fn to_json(&self) -> Result<String, ConfigError> {
        Ok(serde_json::to_string_pretty(&VersionedConfig {
            schema_version: CONFIG_SCHEMA_VERSION,
            config: self,
        })?)
    }

    /// Normalizes modifiers, deduplicates colliding hotkeys, and clamps
    /// out-of-range values so a hand-edited or older config can never put the
    /// app into a broken state.
    pub fn normalize(&mut self) {
        self.placement_modifiers = self.placement_modifiers.normalized();
        if self.placement_modifiers.is_empty() {
            self.placement_modifiers = BASE_MODIFIERS;
        }
        self.display_modifiers = self.display_modifiers.normalized();
        if self.display_modifiers.is_empty() {
            self.display_modifiers = DISPLAY_MODIFIERS;
        }

        // Ensure no two actions resolve to the same active hotkey (for example
        // after a hand edit). Earlier actions in `WindowAction::ALL` win.
        let mut seen = HashSet::new();
        for action in WindowAction::ALL {
            if let Some(hotkey) = self.binding(action) {
                if !seen.insert(hotkey) {
                    self.keys.insert(action, None);
                }
            }
        }

        self.gaps.normalize();
        self.normalize_cycle_sizes();
        self.animation.normalize();
    }

    fn normalize_cycle_sizes(&mut self) {
        self.cycle_sizes.sort_by_key(|s| s.cycle_rank());
        self.cycle_sizes.dedup();
    }

    /// The sizes a repeated press cycles through, in cycle order.
    pub fn cycle_sizes(&self) -> &[CycleSize] {
        &self.cycle_sizes
    }

    /// Whether a repeat should cycle at all.
    pub fn cycles_sizes(&self) -> bool {
        self.subsequent_execution_mode == SubsequentExecutionMode::CycleSizes
            && !self.cycle_sizes.is_empty()
    }

    /// The modifier combination owned by `group`.
    pub const fn group_modifiers(&self, group: ShortcutGroup) -> Modifiers {
        match group {
            ShortcutGroup::Placement => self.placement_modifiers,
            ShortcutGroup::Displays => self.display_modifiers,
        }
    }

    /// The base modifier that enables single-hold corner composition, or `None`
    /// when either no side action (`Left`/`Right`) or no vertical action
    /// (`Maximize`/`Restore`) is bound.
    pub fn compose_modifiers(&self) -> Option<Modifiers> {
        let has_side = self.key(WindowAction::LeftHalf).is_some()
            || self.key(WindowAction::RightHalf).is_some();
        let has_vertical =
            self.key(WindowAction::Maximize).is_some() || self.key(WindowAction::Restore).is_some();
        (has_side && has_vertical).then_some(self.placement_modifiers)
    }

    /// The bare key bound to `action`, if any.
    pub fn key(&self, action: WindowAction) -> Option<KeyCode> {
        self.keys.get(&action).copied().flatten()
    }

    /// The full [`Hotkey`] bound to `action`, combining its group's modifier
    /// with its configured key.
    pub fn binding(&self, action: WindowAction) -> Option<Hotkey> {
        self.key(action)
            .map(|key| Hotkey::new(self.group_modifiers(action.group()), key))
    }

    /// Every action other than `action` that `hotkey` is bound to, in
    /// catalogue order.
    pub fn actions_using(&self, hotkey: Hotkey, action: WindowAction) -> Vec<WindowAction> {
        WindowAction::ALL
            .into_iter()
            .filter(|a| *a != action && self.binding(*a) == Some(hotkey))
            .collect()
    }

    /// Assigns `key` to `action` under its group's modifier, unbinding any
    /// other action whose full hotkey would collide.
    pub fn set_key(&mut self, action: WindowAction, key: Option<KeyCode>) {
        if let Some(k) = key {
            let hk = Hotkey::new(self.group_modifiers(action.group()), k);
            for other in self.actions_using(hk, action) {
                self.keys.insert(other, None);
            }
        }
        self.keys.insert(action, key);
    }

    /// Pairs `(in_group, in_other_group)` that would collide if `group`'s
    /// modifier were changed to `modifiers`.
    pub fn modifier_collisions(
        &self,
        group: ShortcutGroup,
        modifiers: Modifiers,
    ) -> Vec<(WindowAction, WindowAction)> {
        let modifiers = modifiers.normalized();
        let other_group = match group {
            ShortcutGroup::Placement => ShortcutGroup::Displays,
            ShortcutGroup::Displays => ShortcutGroup::Placement,
        };
        if modifiers != self.group_modifiers(other_group) {
            return Vec::new();
        }
        let mut collisions = Vec::new();
        for action in group.actions() {
            let Some(k) = self.key(action) else {
                continue;
            };
            for other in other_group.actions() {
                if self.key(other) == Some(k) {
                    collisions.push((action, other));
                }
            }
        }
        collisions
    }

    /// Updates `group`'s modifier combination. Fails if `modifiers` is empty or
    /// would collide with bound keys in the other group.
    pub fn set_group_modifiers(
        &mut self,
        group: ShortcutGroup,
        modifiers: Modifiers,
    ) -> Result<(), ConfigError> {
        let modifiers = modifiers.normalized();
        if modifiers.is_empty() {
            return Err(ConfigError::Invalid(
                "Choose at least one modifier key.".into(),
            ));
        }
        let collisions = self.modifier_collisions(group, modifiers);
        if !collisions.is_empty() {
            let names: Vec<_> = collisions
                .iter()
                .map(|(a, b)| format!("{} and {}", a.label(), b.label()))
                .collect();
            return Err(ConfigError::Invalid(format!(
                "Cannot use the same modifier for both groups while {} share a key.",
                names.join(", ")
            )));
        }
        match group {
            ShortcutGroup::Placement => self.placement_modifiers = modifiers,
            ShortcutGroup::Displays => self.display_modifiers = modifiers,
        }
        Ok(())
    }

    /// Every currently bound `(hotkey, action)` pair, for backend registration.
    pub fn active_bindings(&self) -> Vec<(Hotkey, WindowAction)> {
        WindowAction::ALL
            .into_iter()
            .filter_map(|action| self.binding(action).map(|hk| (hk, action)))
            .collect()
    }
}

struct MigratedShortcuts {
    placement_modifiers: Modifiers,
    display_modifiers: Modifiers,
    keys: BTreeMap<WindowAction, Option<KeyCode>>,
}

/// Extracts legacy v1 `"bindings"` when present without v2 `"keys"`.
fn take_legacy_bindings(
    map: &mut serde_json::Map<String, serde_json::Value>,
) -> Option<serde_json::Value> {
    if map.contains_key(KEYS_KEY) {
        map.remove(BINDINGS_KEY);
        return None;
    }
    map.remove(BINDINGS_KEY)
}

fn apply_migrated_to_map(
    map: &mut serde_json::Map<String, serde_json::Value>,
    migrated: MigratedShortcuts,
) {
    if !map.contains_key(PLACEMENT_MODIFIERS_KEY) {
        if let Ok(val) = serde_json::to_value(migrated.placement_modifiers) {
            map.insert(PLACEMENT_MODIFIERS_KEY.to_owned(), val);
        }
    }
    if !map.contains_key(DISPLAY_MODIFIERS_KEY) {
        if let Ok(val) = serde_json::to_value(migrated.display_modifiers) {
            map.insert(DISPLAY_MODIFIERS_KEY.to_owned(), val);
        }
    }
    if let Ok(val) = serde_json::to_value(migrated.keys) {
        map.insert(KEYS_KEY.to_owned(), val);
    }
}

/// Converts a legacy v1 `"bindings"` object into v2 group modifiers and keys.
///
/// - Retired action names are dropped silently (logged at info level).
/// - Legacy pre-directional `previous-display`/`next-display` defaults upgrade
///   to `DisplayLeft`/`DisplayRight` plus `DisplayUp`/`DisplayDown`.
/// - Within each group, bindings are unified onto the majority modifier
///   (breaking ties on `LeftHalf` / `DisplayLeft`, then `WindowAction::ALL`),
///   keeping each action's key and unbinding only if two keys collide after
///   unifying.
fn migrate_legacy_bindings(
    raw: &serde_json::Value,
    reset_fields: &mut Vec<String>,
    notes: &mut Vec<String>,
) -> Result<MigratedShortcuts, ConfigError> {
    let serde_json::Value::Object(entries) = raw else {
        return Err(ConfigError::Invalid(
            "legacy bindings must be a JSON object".into(),
        ));
    };

    let mut parsed: BTreeMap<WindowAction, Option<Hotkey>> = BTreeMap::new();
    let mut legacy_prev: Option<Hotkey> = None;
    let mut legacy_next: Option<Hotkey> = None;
    let mut has_directional_display = false;

    for (action_name, val) in entries {
        if let Ok(action) = WindowAction::from_str(action_name) {
            if action.moves_display() {
                has_directional_display = true;
            }
            match serde_json::from_value::<Option<Hotkey>>(val.clone()) {
                Ok(hk) => {
                    let clean = hk.and_then(|h| {
                        let mods = h.modifiers.normalized();
                        (!mods.is_empty()).then_some(Hotkey::new(mods, h.key))
                    });
                    parsed.insert(action, clean);
                }
                Err(_) => {
                    reset_fields.push(format!("{BINDINGS_KEY}.{action_name}"));
                }
            }
        } else if action_name == "previous-display" || action_name == "next-display" {
            if let Ok(Some(hk)) = serde_json::from_value::<Option<Hotkey>>(val.clone()) {
                let mods = hk.modifiers.normalized();
                if !mods.is_empty() {
                    let normalized = Hotkey::new(mods, hk.key);
                    if action_name == "previous-display" {
                        legacy_prev = Some(normalized);
                    } else {
                        legacy_next = Some(normalized);
                    }
                }
            }
        } else {
            notes.push(format!("dropped retired v1 action '{action_name}'"));
        }
    }

    // Upgrade pre-directional v1 configs (including the retired Shift+arrow throws).
    if !has_directional_display && (legacy_prev.is_some() || legacy_next.is_some()) {
        let retired_shift = BASE_MODIFIERS.union(Modifiers::SHIFT);
        let matches_old_default = |hk: Option<Hotkey>, key: KeyCode| {
            hk == Some(Hotkey::new(DISPLAY_MODIFIERS, key))
                || hk == Some(Hotkey::new(retired_shift, key))
        };
        if matches_old_default(legacy_prev, KeyCode::Left) {
            parsed.insert(
                WindowAction::DisplayLeft,
                Some(Hotkey::new(DISPLAY_MODIFIERS, KeyCode::Left)),
            );
        }
        if matches_old_default(legacy_next, KeyCode::Right) {
            parsed.insert(
                WindowAction::DisplayRight,
                Some(Hotkey::new(DISPLAY_MODIFIERS, KeyCode::Right)),
            );
        }
        for (action, key) in [
            (WindowAction::DisplayUp, KeyCode::Up),
            (WindowAction::DisplayDown, KeyCode::Down),
        ] {
            let candidate = Hotkey::new(DISPLAY_MODIFIERS, key);
            let taken = parsed.values().any(|hk| *hk == Some(candidate));
            if !taken {
                parsed.insert(action, Some(candidate));
            }
        }
    }

    let (placement_modifiers, placement_keys) = unify_group(
        &parsed,
        ShortcutGroup::Placement,
        WindowAction::LeftHalf,
        BASE_MODIFIERS,
        notes,
    );
    let (display_modifiers, mut display_keys) = unify_group(
        &parsed,
        ShortcutGroup::Displays,
        WindowAction::DisplayLeft,
        DISPLAY_MODIFIERS,
        notes,
    );

    // Avoid cross-group hotkey collisions if both groups unified onto the same
    // modifier.
    if placement_modifiers == display_modifiers {
        let used: HashSet<KeyCode> = placement_keys.values().copied().flatten().collect();
        for action in ShortcutGroup::Displays.actions() {
            if let Some(k) = display_keys.get(&action).copied().flatten() {
                if used.contains(&k) {
                    notes.push(format!(
                        "unbound {action} during migration: key {k:?} collided with placement group"
                    ));
                    display_keys.insert(action, None);
                }
            }
        }
    }

    let mut keys = BTreeMap::new();
    for action in WindowAction::ALL {
        let key = match action.group() {
            ShortcutGroup::Placement => placement_keys.get(&action).copied().flatten(),
            ShortcutGroup::Displays => display_keys.get(&action).copied().flatten(),
        };
        keys.insert(action, key);
    }

    Ok(MigratedShortcuts {
        placement_modifiers,
        display_modifiers,
        keys,
    })
}

fn unify_group(
    parsed: &BTreeMap<WindowAction, Option<Hotkey>>,
    group: ShortcutGroup,
    tie_breaker: WindowAction,
    default_mods: Modifiers,
    notes: &mut Vec<String>,
) -> (Modifiers, BTreeMap<WindowAction, Option<KeyCode>>) {
    let actions: Vec<WindowAction> = group.actions().collect();
    let bound: Vec<(WindowAction, Hotkey)> = actions
        .iter()
        .filter_map(|&a| parsed.get(&a).copied().flatten().map(|hk| (a, hk)))
        .collect();

    let chosen_mods = if bound.is_empty() {
        default_mods
    } else {
        let mut counts: BTreeMap<u8, usize> = BTreeMap::new();
        for (_, hk) in &bound {
            *counts.entry(hk.modifiers.0).or_insert(0) += 1;
        }
        let max_count = counts.values().copied().max().unwrap_or(0);
        let tie_mods = parsed
            .get(&tie_breaker)
            .copied()
            .flatten()
            .map(|h| h.modifiers);
        if let Some(m) = tie_mods.filter(|m| counts.get(&m.0).copied() == Some(max_count)) {
            m
        } else {
            bound
                .iter()
                .find(|(_, hk)| counts.get(&hk.modifiers.0).copied() == Some(max_count))
                .map(|(_, hk)| hk.modifiers)
                .unwrap_or(default_mods)
        }
    };

    // Actions already on `chosen_mods` claim their key first; unified outliers
    // follow, dropping only if their key collides.
    let mut result: BTreeMap<WindowAction, Option<KeyCode>> =
        actions.iter().map(|&a| (a, None)).collect();
    let mut used_keys: HashSet<KeyCode> = HashSet::new();

    for prefer_matching in [true, false] {
        for &(action, hk) in &bound {
            if (hk.modifiers == chosen_mods) != prefer_matching {
                continue;
            }
            if used_keys.insert(hk.key) {
                result.insert(action, Some(hk.key));
            } else {
                notes.push(format!(
                    "unbound {action} during migration: key {:?} collided under unified modifier {chosen_mods}",
                    hk.key
                ));
            }
        }
    }

    (chosen_mods, result)
}

/// Upper bound for the screen-edge gap, to stop a typo shrinking windows away.
pub const MAX_GAP: f64 = 200.0;

/// Default base modifier for window placement.
///
/// macOS uses `Control+Option`; Windows uses `Win` (avoiding `Ctrl+Alt` which
/// acts as `AltGr` on many international layouts).
#[cfg(target_os = "macos")]
pub const BASE_MODIFIERS: Modifiers = Modifiers(Modifiers::CONTROL.0 | Modifiers::ALT.0);
#[cfg(not(target_os = "macos"))]
pub const BASE_MODIFIERS: Modifiers = Modifiers::META;

/// Default display modifier for moving windows across displays.
///
/// * **Windows** — `Win+Alt`
/// * **macOS** — `Control+Option+Command`
pub const DISPLAY_MODIFIERS: Modifiers =
    Modifiers(BASE_MODIFIERS.0 | Modifiers::ALT.0 | Modifiers::META.0);

/// Default per-action keys paired with [`BASE_MODIFIERS`] and
/// [`DISPLAY_MODIFIERS`].
pub fn default_keys() -> BTreeMap<WindowAction, Option<KeyCode>> {
    let mut map = BTreeMap::new();
    map.insert(WindowAction::LeftHalf, Some(KeyCode::Left));
    map.insert(WindowAction::RightHalf, Some(KeyCode::Right));
    map.insert(WindowAction::Maximize, Some(KeyCode::Up));
    map.insert(WindowAction::Restore, Some(KeyCode::Down));
    map.insert(WindowAction::CenterHalf, Some(KeyCode::Enter));
    map.insert(WindowAction::DisplayLeft, Some(KeyCode::Left));
    map.insert(WindowAction::DisplayRight, Some(KeyCode::Right));
    map.insert(WindowAction::DisplayUp, Some(KeyCode::Up));
    map.insert(WindowAction::DisplayDown, Some(KeyCode::Down));
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_bind_all_nine_actions_without_conflicts_or_game_bar_keys() {
        let cfg = Config::default();
        let active = cfg.active_bindings();
        assert_eq!(active.len(), 9);
        let unique: HashSet<Hotkey> = active.iter().map(|(hk, _)| *hk).collect();
        assert_eq!(unique.len(), 9);
        assert_eq!(cfg.compose_modifiers(), Some(BASE_MODIFIERS));

        const GAME_BAR_KEYS: [KeyCode; 6] = [
            KeyCode::G,
            KeyCode::R,
            KeyCode::M,
            KeyCode::B,
            KeyCode::W,
            KeyCode::T,
        ];
        for (hk, _) in cfg.active_bindings() {
            assert!(hk.is_valid());
            assert!(!GAME_BAR_KEYS.contains(&hk.key));
        }
    }

    #[test]
    fn config_round_trips_through_v2_json() {
        let mut cfg = Config {
            placement_modifiers: Modifiers::CONTROL | Modifiers::META,
            ..Default::default()
        };
        cfg.set_key(WindowAction::LeftHalf, Some(KeyCode::H));
        cfg.set_key(WindowAction::CenterHalf, None);
        cfg.gaps.window = 12.0;

        let json = cfg.to_json().unwrap();
        assert!(json.contains("\"schemaVersion\": 2"));
        let parsed = Config::from_json(&json).unwrap();
        assert_eq!(parsed, cfg);
    }

    #[test]
    fn legacy_v1_config_migrates_silently_dropping_retired_actions_and_advanced_fields() {
        let v1 = serde_json::json!({
            "schemaVersion": 1,
            "bindings": {
                "left-half": { "modifiers": BASE_MODIFIERS.0, "key": "left" },
                "right-half": { "modifiers": BASE_MODIFIERS.0, "key": "right" },
                "maximize": { "modifiers": BASE_MODIFIERS.0, "key": "up" },
                "restore": { "modifiers": BASE_MODIFIERS.0, "key": "down" },
                "center-half": { "modifiers": BASE_MODIFIERS.0, "key": "enter" },
                "display-left": { "modifiers": DISPLAY_MODIFIERS.0, "key": "left" },
                "display-right": { "modifiers": DISPLAY_MODIFIERS.0, "key": "right" },
                "display-up": { "modifiers": DISPLAY_MODIFIERS.0, "key": "up" },
                "display-down": { "modifiers": DISPLAY_MODIFIERS.0, "key": "down" },
                "first-third": { "modifiers": BASE_MODIFIERS.0, "key": "a" },
                "top-left": { "modifiers": BASE_MODIFIERS.0, "key": "u" },
                "move-left": null
            },
            "almostMaximizeWidth": 0.85,
            "sizeStep": 40,
            "animation": { "enabled": true, "durationMs": 200, "fps": 120 }
        })
        .to_string();

        let loaded = Config::from_json_lenient(&v1).unwrap();
        assert!(
            loaded.reset_fields.is_empty(),
            "retired fields must migrate silently, got {:?}",
            loaded.reset_fields
        );
        assert_eq!(loaded.config.placement_modifiers, BASE_MODIFIERS);
        assert_eq!(loaded.config.display_modifiers, DISPLAY_MODIFIERS);
        assert_eq!(
            loaded.config.key(WindowAction::LeftHalf),
            Some(KeyCode::Left)
        );
        assert_eq!(
            loaded.config.key(WindowAction::CenterHalf),
            Some(KeyCode::Enter)
        );
        assert_eq!(
            loaded.config.key(WindowAction::DisplayUp),
            Some(KeyCode::Up)
        );
        assert_eq!(loaded.config.animation.duration_ms, 200);
    }

    #[test]
    fn legacy_v1_mixed_modifiers_unify_by_majority_with_left_tiebreak_and_drop_only_collisions() {
        let m1 = Modifiers::CONTROL | Modifiers::SHIFT;
        let m2 = Modifiers::CONTROL | Modifiers::ALT;
        let v1 = serde_json::json!({
            "schemaVersion": 1,
            "bindings": {
                "left-half": { "modifiers": m1.0, "key": "h" },
                "right-half": { "modifiers": m1.0, "key": "l" },
                // Different modifier, distinct key -> kept under m1.
                "maximize": { "modifiers": m2.0, "key": "k" },
                // Different modifier, colliding key with left-half -> unbound.
                "restore": { "modifiers": m2.0, "key": "h" },
                // Tie in display group: DisplayLeft wins tie-break.
                "display-left": { "modifiers": m2.0, "key": "left" },
                "display-right": { "modifiers": m1.0, "key": "right" }
            }
        })
        .to_string();

        let loaded = Config::from_json_lenient(&v1).unwrap();
        assert!(loaded.reset_fields.is_empty());
        assert_eq!(loaded.config.placement_modifiers, m1);
        assert_eq!(loaded.config.key(WindowAction::LeftHalf), Some(KeyCode::H));
        assert_eq!(loaded.config.key(WindowAction::RightHalf), Some(KeyCode::L));
        assert_eq!(loaded.config.key(WindowAction::Maximize), Some(KeyCode::K));
        assert_eq!(loaded.config.key(WindowAction::Restore), None);
        assert_eq!(loaded.config.key(WindowAction::CenterHalf), None);

        assert_eq!(loaded.config.display_modifiers, m2);
        assert_eq!(
            loaded.config.key(WindowAction::DisplayLeft),
            Some(KeyCode::Left)
        );
        assert_eq!(
            loaded.config.key(WindowAction::DisplayRight),
            Some(KeyCode::Right)
        );
    }

    #[test]
    fn legacy_pre_directional_display_bindings_upgrade_to_directional() {
        let retired = BASE_MODIFIERS.union(Modifiers::SHIFT);
        let v1 = serde_json::json!({
            "bindings": {
                "left-half": { "modifiers": BASE_MODIFIERS.0, "key": "left" },
                "previous-display": { "modifiers": retired.0, "key": "left" },
                "next-display": { "modifiers": retired.0, "key": "right" }
            }
        })
        .to_string();

        let loaded = Config::from_json_lenient(&v1).unwrap();
        assert!(loaded.reset_fields.is_empty());
        assert_eq!(loaded.config.display_modifiers, DISPLAY_MODIFIERS);
        assert_eq!(
            loaded.config.key(WindowAction::DisplayLeft),
            Some(KeyCode::Left)
        );
        assert_eq!(
            loaded.config.key(WindowAction::DisplayRight),
            Some(KeyCode::Right)
        );
        assert_eq!(
            loaded.config.key(WindowAction::DisplayUp),
            Some(KeyCode::Up)
        );
        assert_eq!(
            loaded.config.key(WindowAction::DisplayDown),
            Some(KeyCode::Down)
        );
    }

    #[test]
    fn equal_modifiers_allowed_when_keys_are_distinct_and_blocked_on_collision() {
        let mut cfg = Config::default();
        // Default keys share Left/Right/Up/Down across both groups -> blocked.
        assert!(cfg
            .set_group_modifiers(ShortcutGroup::Displays, BASE_MODIFIERS)
            .is_err());

        // Move display keys to 1/2/3/4 -> sharing the modifier now succeeds.
        cfg.set_key(WindowAction::DisplayLeft, Some(KeyCode::Digit1));
        cfg.set_key(WindowAction::DisplayRight, Some(KeyCode::Digit2));
        cfg.set_key(WindowAction::DisplayUp, Some(KeyCode::Digit3));
        cfg.set_key(WindowAction::DisplayDown, Some(KeyCode::Digit4));
        assert!(cfg
            .set_group_modifiers(ShortcutGroup::Displays, BASE_MODIFIERS)
            .is_ok());
        assert_eq!(cfg.display_modifiers, BASE_MODIFIERS);
        let unique: HashSet<Hotkey> = cfg
            .active_bindings()
            .into_iter()
            .map(|(hk, _)| hk)
            .collect();
        assert_eq!(unique.len(), 9);

        // Setting a key in Displays to Left now sees LeftHalf as holder across groups.
        let hk = Hotkey::new(BASE_MODIFIERS, KeyCode::Left);
        assert_eq!(
            cfg.actions_using(hk, WindowAction::DisplayLeft),
            vec![WindowAction::LeftHalf]
        );
    }

    #[test]
    fn compose_modifiers_disables_when_side_or_vertical_actions_are_unbound() {
        let mut cfg = Config::default();
        assert_eq!(cfg.compose_modifiers(), Some(BASE_MODIFIERS));
        cfg.set_key(WindowAction::LeftHalf, None);
        assert_eq!(cfg.compose_modifiers(), Some(BASE_MODIFIERS));
        cfg.set_key(WindowAction::RightHalf, None);
        assert_eq!(cfg.compose_modifiers(), None);
    }

    #[test]
    fn animation_params_uses_refresh_rate_with_fallback_and_clamping() {
        let cfg = AnimationConfig::default();
        assert_eq!(cfg.params(None).fps, DEFAULT_ANIMATION_FPS);
        assert_eq!(cfg.params(Some(0)).fps, DEFAULT_ANIMATION_FPS);
        assert_eq!(cfg.params(Some(120)).fps, 120);
        assert_eq!(cfg.params(Some(500)).fps, MAX_ANIMATION_FPS);
        assert_eq!(cfg.params(Some(5)).fps, MIN_ANIMATION_FPS);
    }
}
