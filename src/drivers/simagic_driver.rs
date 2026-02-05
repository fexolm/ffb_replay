//! Direct HID FFB driver for SIMAGIC wheelbases
//! 
//! This driver sends FFB commands directly via HID, bypassing SDL.
//! Protocol reverse-engineered from USB packet captures.

use crate::{
    driver::FfbDriver,
    effects::*,
    error::{FFBError, FFBResult},
};
use std::fs::File;
use std::io::Write;

/// HID Report structure for SIMAGIC FFB commands
/// All reports are 21 bytes with Report ID 0x01
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct FfbReport {
    pub report_id: u8,       // Always 0x01
    pub command: u8,         // Command type
    pub effect_type: u8,     // Effect type
    pub data: [u8; 18],      // Command-specific data
}

impl Default for FfbReport {
    fn default() -> Self {
        Self {
            report_id: 0x01,
            command: 0x00,
            effect_type: 0x00,
            data: [0u8; 18],
        }
    }
}

impl FfbReport {
    pub fn to_bytes(&self) -> [u8; 21] {
        let mut bytes = [0u8; 21];
        bytes[0] = self.report_id;
        bytes[1] = self.command;
        bytes[2] = self.effect_type;
        bytes[3..21].copy_from_slice(&self.data);
        bytes
    }
}

/// Command types for SIMAGIC FFB protocol
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum FfbCommand {
    SetEffect = 0x01,           // Set effect parameters (duration, etc.)
    SetConditionParams = 0x03,  // Set condition effect parameters
    SetConstantMagnitude = 0x05, // Set constant force magnitude
    StartEffect = 0x0A,         // Start/run effect
    StopEffect = 0x0B,          // Stop effect (assumed)
}

/// Effect types in SIMAGIC FFB protocol
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimagicEffectType {
    Constant = 0x01,
    Sine = 0x02,
    // 0x03, 0x04 - unknown
    Damper = 0x05,
    Spring = 0x06,
    // 0x07-0x0D - unknown
    Ramp = 0x0E,
    Square = 0x0F,
    Triangle = 0x10,
    SawtoothUp = 0x11,   // Assumed
    SawtoothDown = 0x12, // Assumed
    Friction = 0x07,     // Confirmed from captures
    Inertia = 0x09,      // Confirmed from captures (not 0x08)
}

impl From<&Effect> for SimagicEffectType {
    fn from(effect: &Effect) -> Self {
        match effect {
            Effect::Constant { .. } => SimagicEffectType::Constant,
            Effect::Periodic { effect, .. } => match effect.wave_type {
                WaveType::Sine => SimagicEffectType::Sine,
                WaveType::Square => SimagicEffectType::Square,
                WaveType::Triangle => SimagicEffectType::Triangle,
                WaveType::SawtoothUp => SimagicEffectType::SawtoothUp,
                WaveType::SawtoothDown => SimagicEffectType::SawtoothDown,
            },
            Effect::Ramp { .. } => SimagicEffectType::Ramp,
            Effect::Condition { effect, .. } => match effect.condition_type {
                ConditionType::Spring => SimagicEffectType::Spring,
                ConditionType::Damper => SimagicEffectType::Damper,
                ConditionType::Friction => SimagicEffectType::Friction,
                ConditionType::Inertia => SimagicEffectType::Inertia,
            },
        }
    }
}

/// Direct HID FFB device driver
pub struct SimagicDriver {
    /// Current effect slot
    current_effect_slot: u8,
    /// Whether device is initialized
    initialized: bool,
}

impl SimagicDriver {
    pub fn new() -> Self {
        Self {
            current_effect_slot: 1,
            initialized: false,
        }
    }

    /// Create SET_EFFECT command (0x01)
    fn create_set_effect_report(&self, effect_type: SimagicEffectType, duration_ms: u32) -> [u8; 21] {
        let mut report = FfbReport::default();
        report.command = FfbCommand::SetEffect as u8;
        report.effect_type = effect_type as u8;
        
        // Byte 3: Effect slot (always 0x01 for now)
        report.data[0] = 0x01;
        
        // Bytes 4-5: Duration (little-endian, in ms)
        let duration = duration_ms.min(0xFFFF) as u16;
        report.data[1] = (duration & 0xFF) as u8;
        report.data[2] = ((duration >> 8) & 0xFF) as u8;
        
        // Bytes 6-7: Start delay (0 for now)
        report.data[3] = 0x00;
        report.data[4] = 0x00;
        
        // Bytes 8-9: Unknown (0x00 0x00)
        report.data[5] = 0x00;
        report.data[6] = 0x00;
        
        // Bytes 10-11: Unknown (0xFF 0xFF)
        report.data[7] = 0xFF;
        report.data[8] = 0xFF;
        
        // Bytes 12-13: Unknown (0x04 0x3F - possibly gain/direction)
        report.data[9] = 0x04;
        report.data[10] = 0x3F;
        
        // Rest is zeros
        
        report.to_bytes()
    }

    /// Create SET_CONSTANT_MAGNITUDE command (0x05)
    fn create_set_constant_magnitude_report(&self, effect_slot: u8, magnitude: i16) -> [u8; 21] {
        let mut report = FfbReport::default();
        report.command = FfbCommand::SetConstantMagnitude as u8;
        report.effect_type = effect_slot;
        
        // Driver uses nearly 1:1 mapping with adjustments:
        // - magnitude 1 -> 0 (due to SDL scaling rounding)
        // - magnitude ±10000 -> ±10000 (max values unchanged)
        // - other values: ±1 adjustment towards zero
        let adjusted = if magnitude == 1 {
            0 // SDL scaling: 1 * 32767/10000 = 3, then back: 3 * 10000/32767 ≈ 0
        } else if magnitude == 10000 || magnitude == -10000 || magnitude == 0 {
            magnitude
        } else if magnitude > 0 {
            magnitude.saturating_sub(1)
        } else {
            magnitude.saturating_add(1)
        };
        report.data[0] = (adjusted & 0xFF) as u8;
        report.data[1] = ((adjusted >> 8) & 0xFF) as u8;
        
        report.to_bytes()
    }

    /// Create SET_CONDITION_PARAMS command (0x03)
    fn create_set_condition_params_report(
        &self,
        effect_type: SimagicEffectType,
        params: &ConditionParams,
    ) -> [u8; 21] {
        let mut report = FfbReport::default();
        report.command = FfbCommand::SetConditionParams as u8;
        report.effect_type = effect_type as u8;
        
        // Byte 3: Padding (0x00)
        report.data[0] = 0x00;
        
        // Bytes 4-5: Offset (scaled: offset / 3.28, little-endian, round up)
        let offset_scaled = (params.offset as f32) / 3.28;
        let offset = if params.offset >= 0 {
            offset_scaled.ceil() as i16
        } else {
            offset_scaled.floor() as i16
        };
        report.data[1] = (offset & 0xFF) as u8;
        report.data[2] = ((offset >> 8) & 0xFF) as u8;
        
        // Bytes 6-7: Positive coefficient (little-endian)
        let pos_coeff = if params.positive_coefficient == 0 || params.positive_coefficient >= 10000 {
            params.positive_coefficient as i16
        } else {
            (params.positive_coefficient - 1) as i16
        };
        report.data[3] = (pos_coeff & 0xFF) as u8;
        report.data[4] = ((pos_coeff >> 8) & 0xFF) as u8;
        
        // Bytes 8-9: Negative coefficient (little-endian)
        let neg_coeff = if params.negative_coefficient == 0 || params.negative_coefficient >= 10000 {
            params.negative_coefficient as i16
        } else {
            (params.negative_coefficient - 1) as i16
        };
        report.data[5] = (neg_coeff & 0xFF) as u8;
        report.data[6] = ((neg_coeff >> 8) & 0xFF) as u8;
        
        // Bytes 10-11: Positive saturation (little-endian)
        let pos_sat = (params.positive_saturation / 2).saturating_sub(1);
        report.data[7] = (pos_sat & 0xFF) as u8;
        report.data[8] = ((pos_sat >> 8) & 0xFF) as u8;
        
        // Bytes 12-13: Negative saturation (little-endian)
        let neg_sat = (params.negative_saturation / 2).saturating_sub(1);
        report.data[9] = (neg_sat & 0xFF) as u8;
        report.data[10] = ((neg_sat >> 8) & 0xFF) as u8;
        
        // Bytes 14-15: Dead band (16-bit little-endian, scaled: dead_band / 6.56, round up)
        let dead_band = ((params.dead_band as f32) / 6.56).ceil() as u16;
        report.data[11] = (dead_band & 0xFF) as u8;
        report.data[12] = ((dead_band >> 8) & 0xFF) as u8;
        
        // Rest is zeros
        
        report.to_bytes()
    }

    /// Create START_EFFECT command (0x0A)
    fn create_start_effect_report(&self, effect_type: SimagicEffectType, effect_slot: u8) -> [u8; 21] {
        let mut report = FfbReport::default();
        report.command = FfbCommand::StartEffect as u8;
        report.effect_type = effect_type as u8;
        
        // Byte 3: Effect slot
        report.data[0] = effect_slot;
        
        // Byte 4: Play count (0x01 = play once)
        report.data[1] = 0x01;
        
        report.to_bytes()
    }

    /// Create STOP_EFFECT command (assumed 0x0B)
    #[allow(dead_code)]
    fn create_stop_effect_report(&self, effect_type: SimagicEffectType, effect_slot: u8) -> [u8; 21] {
        let mut report = FfbReport::default();
        report.command = FfbCommand::StopEffect as u8;
        report.effect_type = effect_type as u8;
        report.data[0] = effect_slot;
        report.to_bytes()
    }

    /// Format report as hex string for display
    pub fn format_report(report: &[u8; 21]) -> String {
        report.iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl Default for SimagicDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl FfbDriver for SimagicDriver {
    fn initialize(&mut self) -> FFBResult<()> {
        // For now, we just mark as initialized
        // Real implementation would enumerate HID devices and find SIMAGIC
        println!("SIMAGIC HID FFB device initialized (simulation mode)");
        self.initialized = true;
        Ok(())
    }

    fn apply_effect(&mut self, effect: &Effect) -> FFBResult<Vec<String>> {
        if !self.initialized {
            return Err(FFBError::DeviceError("Device not initialized".to_string()));
        }

        let effect_type = SimagicEffectType::from(effect);
        let mut generated_reports: Vec<[u8; 21]> = Vec::new();
        
        // Generate reports based on effect type
        match effect {
            Effect::Constant { params, force } => {
                // Driver behavior for magnitude:
                // - magnitude 0: skips SET_CONSTANT_MAGNITUDE
                // - magnitude 1: sends SET_CONSTANT_MAGNITUDE with value 0
                // - magnitude -1: skips SET_CONSTANT_MAGNITUDE  
                // - other values: sends SET_CONSTANT_MAGNITUDE with adjusted value
                if force.magnitude != 0 && force.magnitude != -1 {
                    let magnitude_report = self.create_set_constant_magnitude_report(
                        self.current_effect_slot,
                        force.magnitude,
                    );
                    generated_reports.push(magnitude_report);
                }
                
                // 2. Set effect parameters
                let effect_report = self.create_set_effect_report(effect_type, params.duration);
                generated_reports.push(effect_report);
                
                // 3. Start effect
                let start_report = self.create_start_effect_report(effect_type, self.current_effect_slot);
                generated_reports.push(start_report);
            }
            
            Effect::Periodic { params, effect: _periodic } => {
                // For periodic effects, we just set effect params and start
                // The magnitude/period might be embedded in the SET_EFFECT command
                // or there might be additional commands we haven't captured
                
                // 1. Set effect parameters
                let effect_report = self.create_set_effect_report(effect_type, params.duration);
                generated_reports.push(effect_report);
                
                // 2. Start effect
                let start_report = self.create_start_effect_report(effect_type, self.current_effect_slot);
                generated_reports.push(start_report);
            }
            
            Effect::Ramp { params, effect: _ramp } => {
                // 1. Set effect parameters
                let effect_report = self.create_set_effect_report(effect_type, params.duration);
                generated_reports.push(effect_report);
                
                // 2. Start effect
                let start_report = self.create_start_effect_report(effect_type, self.current_effect_slot);
                generated_reports.push(start_report);
            }
            
            Effect::Condition { params, effect: condition } => {
                // 1. Set condition parameters
                let condition_report = self.create_set_condition_params_report(
                    effect_type,
                    &condition.x_axis,
                );
                generated_reports.push(condition_report);
                
                // 2. Set effect parameters
                let effect_report = self.create_set_effect_report(effect_type, params.duration);
                generated_reports.push(effect_report);
                
                // 3. Start effect
                let start_report = self.create_start_effect_report(effect_type, self.current_effect_slot);
                generated_reports.push(start_report);
            }
        }
        
        // Return reports as hex strings
        Ok(generated_reports.iter().map(Self::format_report).collect())
    }

    fn stop_all_effects(&mut self) -> FFBResult<()> {
        // Send stop commands for common effect types
        // In practice, we'd track which effects are active
        Ok(())
    }

    fn shutdown(&mut self) -> FFBResult<()> {
        self.stop_all_effects()?;
        self.initialized = false;
        Ok(())
    }
    
    fn name(&self) -> &str {
        "SIMAGIC"
    }
    
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Compare two reports and show differences
#[allow(dead_code)]
pub fn compare_reports(expected: &[u8; 21], actual: &[u8; 21]) -> (bool, String) {
    let mut differences = Vec::new();
    let mut match_count = 0;
    
    for i in 0..21 {
        if expected[i] == actual[i] {
            match_count += 1;
        } else {
            differences.push(format!("byte {}: expected {:02X}, got {:02X}", i, expected[i], actual[i]));
        }
    }
    
    let matches = differences.is_empty();
    let report = if matches {
        format!("OK: All 21 bytes match")
    } else {
        format!("FAIL: {}/{} bytes match. Differences:\n  {}", 
            match_count, 21, differences.join("\n  "))
    };
    
    (matches, report)
}
