use serde::{Deserialize, Serialize};

/// Effect direction (in degrees, 0-360)
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Direction(pub u16);

impl Default for Direction {
    fn default() -> Self {
        Direction(0)
    }
}

/// Envelope for smooth attack and fade of effect
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Envelope {
    /// Attack time (ms)
    pub attack_time: u32,
    /// Level at start of attack (0-10000)
    pub attack_level: u16,
    /// Fade time (ms)
    pub fade_time: u32,
    /// Level at end of fade (0-10000)
    pub fade_level: u16,
}

impl Default for Envelope {
    fn default() -> Self {
        Envelope {
            attack_time: 0,
            attack_level: 0,
            fade_time: 0,
            fade_level: 0,
        }
    }
}

/// Constant force
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstantForce {
    /// Force magnitude (-10000 to 10000)
    pub magnitude: i16,
    /// Direction
    #[serde(default)]
    pub direction: Direction,
    /// Envelope
    #[serde(default)]
    pub envelope: Envelope,
}

/// Periodic wave types
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaveType {
    Sine,
    Square,
    Triangle,
    SawtoothUp,
    SawtoothDown,
}

/// Periodic effect
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeriodicEffect {
    /// Wave type
    pub wave_type: WaveType,
    /// Amplitude (0-10000)
    pub magnitude: u16,
    /// Offset (-10000 to 10000)
    #[serde(default)]
    pub offset: i16,
    /// Phase (0-36000, in hundredths of a degree)
    #[serde(default)]
    pub phase: u16,
    /// Period (ms)
    pub period: u32,
    /// Direction
    #[serde(default)]
    pub direction: Direction,
    /// Envelope
    #[serde(default)]
    pub envelope: Envelope,
}

/// Ramp effect (linear force change)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RampEffect {
    /// Start force (-10000 to 10000)
    pub start_magnitude: i16,
    /// End force (-10000 to 10000)
    pub end_magnitude: i16,
    /// Direction
    #[serde(default)]
    pub direction: Direction,
    /// Envelope
    #[serde(default)]
    pub envelope: Envelope,
}

/// Condition effects (depend on wheel position/velocity)
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionType {
    Spring,   // Spring
    Damper,   // Damper
    Friction, // Friction
    Inertia,  // Inertia
}

/// Condition effect parameters for one axis
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ConditionParams {
    /// Center offset (-10000 to 10000)
    #[serde(default)]
    pub offset: i16,
    /// Positive direction coefficient (-10000 to 10000)
    #[serde(default = "default_coefficient")]
    pub positive_coefficient: i16,
    /// Negative direction coefficient (-10000 to 10000)
    #[serde(default = "default_coefficient")]
    pub negative_coefficient: i16,
    /// Positive direction saturation (0-10000)
    #[serde(default = "default_saturation")]
    pub positive_saturation: u16,
    /// Negative direction saturation (0-10000)
    #[serde(default = "default_saturation")]
    pub negative_saturation: u16,
    /// Dead band (0-10000)
    #[serde(default)]
    pub dead_band: u16,
}

fn default_coefficient() -> i16 {
    10000
}

fn default_saturation() -> u16 {
    10000
}

impl Default for ConditionParams {
    fn default() -> Self {
        ConditionParams {
            offset: 0,
            positive_coefficient: 10000,
            negative_coefficient: 10000,
            positive_saturation: 10000,
            negative_saturation: 10000,
            dead_band: 0,
        }
    }
}

/// Condition effect
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionEffect {
    /// Condition effect type
    pub condition_type: ConditionType,
    /// X axis parameters (usually steering wheel)
    #[serde(default)]
    pub x_axis: ConditionParams,
}

/// Common effect parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectParams {
    /// Effect duration (ms), 0 = infinite
    #[serde(default)]
    pub duration: u32,
    /// Delay before start (ms)
    #[serde(default)]
    pub start_delay: u32,
    /// Gain (0-10000)
    #[serde(default = "default_gain")]
    pub gain: u16,
}

fn default_gain() -> u16 {
    10000
}

impl Default for EffectParams {
    fn default() -> Self {
        EffectParams {
            duration: 1000,
            start_delay: 0,
            gain: 10000,
        }
    }
}

/// All effect types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Effect {
    Constant {
        #[serde(flatten)]
        params: EffectParams,
        #[serde(flatten)]
        force: ConstantForce,
    },
    Periodic {
        #[serde(flatten)]
        params: EffectParams,
        #[serde(flatten)]
        effect: PeriodicEffect,
    },
    Ramp {
        #[serde(flatten)]
        params: EffectParams,
        #[serde(flatten)]
        effect: RampEffect,
    },
    Condition {
        #[serde(flatten)]
        params: EffectParams,
        #[serde(flatten)]
        effect: ConditionEffect,
    },
}

impl Effect {
    pub fn duration(&self) -> u32 {
        match self {
            Effect::Constant { params, .. } => params.duration,
            Effect::Periodic { params, .. } => params.duration,
            Effect::Ramp { params, .. } => params.duration,
            Effect::Condition { params, .. } => params.duration,
        }
    }
    
    pub fn start_delay(&self) -> u32 {
        match self {
            Effect::Constant { params, .. } => params.start_delay,
            Effect::Periodic { params, .. } => params.start_delay,
            Effect::Ramp { params, .. } => params.start_delay,
            Effect::Condition { params, .. } => params.start_delay,
        }
    }
}
