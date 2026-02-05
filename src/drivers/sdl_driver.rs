use crate::{
    driver::FfbDriver,
    effects::*,
    error::{FFBError, FFBResult},
    usb_monitor::{format_hex, UsbMonitor},
};
use sdl3_sys::error::SDL_GetError;
use sdl3_sys::haptic::*;
use sdl3_sys::init::*;
use sdl3_sys::joystick::*;
use sdl3_sys::stdinc::SDL_free;
use std::ffi::CStr;
use std::ptr;
use std::thread;
use std::time::Duration;

// SDL uses range -32767..32767, our config uses -10000..10000
const SCALE_FACTOR: f32 = 32767.0 / 10000.0;

fn scale_magnitude(value: i16) -> i16 {
    ((value as f32) * SCALE_FACTOR).clamp(-32767.0, 32767.0) as i16
}

fn scale_magnitude_u16(value: u16) -> i16 {
    ((value as f32) * SCALE_FACTOR).clamp(0.0, 32767.0) as i16
}

pub struct SdlDriver {
    haptic: *mut SDL_Haptic,
    current_effect_id: Option<SDL_HapticEffectID>,
    initialized: bool,
    usb_monitor: UsbMonitor,
}

impl SdlDriver {
    pub fn new() -> Self {
        SdlDriver {
            haptic: ptr::null_mut(),
            current_effect_id: None,
            initialized: false,
            usb_monitor: UsbMonitor::new(),
        }
    }

    fn map_direction(direction: &Direction) -> SDL_HapticDirection {
        SDL_HapticDirection {
            r#type: SDL_HAPTIC_CARTESIAN,
            dir: [direction.0 as i32 * 100, 0, 0],
        }
    }

    fn create_constant_effect(&self, params: &EffectParams, force: &ConstantForce) -> SDL_HapticEffect {
        let mut effect: SDL_HapticEffect = unsafe { std::mem::zeroed() };
        
        // SAFETY: Writing to union fields requires unsafe
        effect.r#type = SDL_HAPTIC_CONSTANT;
        effect.constant.direction = Self::map_direction(&force.direction);
        effect.constant.length = if params.duration == 0 {
            SDL_HAPTIC_INFINITY
        } else {
            params.duration
        };
        effect.constant.delay = params.start_delay as u16;
        effect.constant.level = scale_magnitude(force.magnitude);
        
        // Envelope
        effect.constant.attack_length = force.envelope.attack_time as u16;
        effect.constant.attack_level = scale_magnitude_u16(force.envelope.attack_level) as u16;
        effect.constant.fade_length = force.envelope.fade_time as u16;
        effect.constant.fade_level = scale_magnitude_u16(force.envelope.fade_level) as u16;
        
        effect
    }

    fn create_periodic_effect(&self, params: &EffectParams, periodic: &PeriodicEffect) -> SDL_HapticEffect {
        let mut effect: SDL_HapticEffect = unsafe { std::mem::zeroed() };
        
        let wave_type = match periodic.wave_type {
            WaveType::Sine => SDL_HAPTIC_SINE,
            WaveType::Square => SDL_HAPTIC_SQUARE,
            WaveType::Triangle => SDL_HAPTIC_TRIANGLE,
            WaveType::SawtoothUp => SDL_HAPTIC_SAWTOOTHUP,
            WaveType::SawtoothDown => SDL_HAPTIC_SAWTOOTHDOWN,
        };
        
        effect.r#type = wave_type;
        effect.periodic.direction = Self::map_direction(&periodic.direction);
        effect.periodic.length = if params.duration == 0 {
            SDL_HAPTIC_INFINITY
        } else {
            params.duration
        };
        effect.periodic.delay = params.start_delay as u16;
        effect.periodic.period = periodic.period as u16;
        effect.periodic.magnitude = scale_magnitude_u16(periodic.magnitude);
        effect.periodic.offset = scale_magnitude(periodic.offset);
        effect.periodic.phase = periodic.phase;
        
        // Envelope
        effect.periodic.attack_length = periodic.envelope.attack_time as u16;
        effect.periodic.attack_level = scale_magnitude_u16(periodic.envelope.attack_level) as u16;
        effect.periodic.fade_length = periodic.envelope.fade_time as u16;
        effect.periodic.fade_level = scale_magnitude_u16(periodic.envelope.fade_level) as u16;
        
        effect
    }

    fn create_ramp_effect(&self, params: &EffectParams, ramp: &RampEffect) -> SDL_HapticEffect {
        let mut effect: SDL_HapticEffect = unsafe { std::mem::zeroed() };
        
        effect.r#type = SDL_HAPTIC_RAMP;
        effect.ramp.direction = Self::map_direction(&ramp.direction);
        effect.ramp.length = if params.duration == 0 {
            SDL_HAPTIC_INFINITY
        } else {
            params.duration
        };
        effect.ramp.delay = params.start_delay as u16;
        effect.ramp.start = scale_magnitude(ramp.start_magnitude);
        effect.ramp.end = scale_magnitude(ramp.end_magnitude);
        
        // Envelope
        effect.ramp.attack_length = ramp.envelope.attack_time as u16;
        effect.ramp.attack_level = scale_magnitude_u16(ramp.envelope.attack_level) as u16;
        effect.ramp.fade_length = ramp.envelope.fade_time as u16;
        effect.ramp.fade_level = scale_magnitude_u16(ramp.envelope.fade_level) as u16;
        
        effect
    }

    fn create_condition_effect(&self, params: &EffectParams, condition: &ConditionEffect) -> SDL_HapticEffect {
        let mut effect: SDL_HapticEffect = unsafe { std::mem::zeroed() };
        
        let cond_type = match condition.condition_type {
            ConditionType::Spring => SDL_HAPTIC_SPRING,
            ConditionType::Damper => SDL_HAPTIC_DAMPER,
            ConditionType::Friction => SDL_HAPTIC_FRICTION,
            ConditionType::Inertia => SDL_HAPTIC_INERTIA,
        };
        
        effect.r#type = cond_type;
        effect.condition.direction.r#type = SDL_HAPTIC_CARTESIAN;
        effect.condition.direction.dir = [0, 0, 0];
        effect.condition.length = if params.duration == 0 {
            SDL_HAPTIC_INFINITY
        } else {
            params.duration
        };
        effect.condition.delay = params.start_delay as u16;
        
        // X axis condition - unsafe needed for array access via union
        // SAFETY: effect was zeroed and we're writing known values
        unsafe {
            effect.condition.right_sat[0] = scale_magnitude_u16(condition.x_axis.positive_saturation) as u16;
            effect.condition.left_sat[0] = scale_magnitude_u16(condition.x_axis.negative_saturation) as u16;
            effect.condition.right_coeff[0] = scale_magnitude(condition.x_axis.positive_coefficient);
            effect.condition.left_coeff[0] = scale_magnitude(condition.x_axis.negative_coefficient);
            effect.condition.deadband[0] = condition.x_axis.dead_band;
            effect.condition.center[0] = condition.x_axis.offset;
        }
        
        effect
    }
    
    fn get_sdl_error() -> String {
        unsafe {
            let error = SDL_GetError();
            if !error.is_null() {
                CStr::from_ptr(error).to_string_lossy().into_owned()
            } else {
                "Unknown error".to_string()
            }
        }
    }
}

impl Default for SdlDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl FfbDriver for SdlDriver {
    fn initialize(&mut self) -> FFBResult<()> {
        // Start USB capture first - this is required
        println!("Starting USB capture...");
        self.usb_monitor.start_capture().map_err(|e| {
            FFBError::InitializationFailed(format!(
                "Failed to start USB capture: {}. Install USBPcap (Windows) or tcpdump (Linux).",
                e
            ))
        })?;

        unsafe {
            // Initialize SDL with joystick and haptic support
            if !SDL_Init(SDL_INIT_JOYSTICK | SDL_INIT_HAPTIC) {
                return Err(FFBError::InitializationFailed(format!(
                    "SDL_Init failed: {}",
                    Self::get_sdl_error()
                )));
            }

            // Search for device with haptic support
            let joysticks = SDL_GetJoysticks(ptr::null_mut());
            if joysticks.is_null() {
                return Err(FFBError::DeviceNotFound);
            }

            let mut found_joystick: *mut SDL_Joystick = ptr::null_mut();
            let mut idx = 0;
            
            loop {
                let joy_id = *joysticks.add(idx);
                if joy_id == 0 {
                    break;
                }
                
                let joystick = SDL_OpenJoystick(joy_id);
                if !joystick.is_null() {
                    if SDL_IsJoystickHaptic(joystick) {
                        found_joystick = joystick;
                        let name = SDL_GetJoystickName(joystick);
                        if !name.is_null() {
                            let name_str = CStr::from_ptr(name).to_string_lossy();
                            println!("Found FFB joystick: {}", name_str);
                        }
                        break;
                    }
                    SDL_CloseJoystick(joystick);
                }
                idx += 1;
            }
            
            SDL_free(joysticks as *mut _);

            if found_joystick.is_null() {
                // Try to open haptic device directly
                let haptics = SDL_GetHaptics(ptr::null_mut());
                if !haptics.is_null() {
                    let first_haptic_id = *haptics;
                    SDL_free(haptics as *mut _);
                    
                    if first_haptic_id != 0 {
                        self.haptic = SDL_OpenHaptic(first_haptic_id);
                        if self.haptic.is_null() {
                            return Err(FFBError::DeviceNotFound);
                        }
                    } else {
                        return Err(FFBError::DeviceNotFound);
                    }
                } else {
                    return Err(FFBError::DeviceNotFound);
                }
            } else {
                self.haptic = SDL_OpenHapticFromJoystick(found_joystick);
                if self.haptic.is_null() {
                    return Err(FFBError::InitializationFailed(format!(
                        "SDL_OpenHapticFromJoystick failed: {}",
                        Self::get_sdl_error()
                    )));
                }
            }

            // Print device info
            let name = SDL_GetHapticName(self.haptic);
            if !name.is_null() {
                let name_str = CStr::from_ptr(name).to_string_lossy();
                println!("Haptic device: {}", name_str);
            }
            
            let num_axes = SDL_GetNumHapticAxes(self.haptic);
            println!("  Axes: {}", num_axes);
            
            let features = SDL_GetHapticFeatures(self.haptic);
            println!("  Supported effects:");
            if (features & SDL_HAPTIC_CONSTANT.0 as u32) != 0 {
                println!("    - Constant force");
            }
            if (features & SDL_HAPTIC_SINE.0 as u32) != 0 {
                println!("    - Periodic (sine, square, triangle, sawtooth)");
            }
            if (features & SDL_HAPTIC_RAMP.0 as u32) != 0 {
                println!("    - Ramp");
            }
            if (features & SDL_HAPTIC_SPRING.0 as u32) != 0 {
                println!("    - Spring");
            }
            if (features & SDL_HAPTIC_DAMPER.0 as u32) != 0 {
                println!("    - Damper");
            }
            if (features & SDL_HAPTIC_FRICTION.0 as u32) != 0 {
                println!("    - Friction");
            }
            if (features & SDL_HAPTIC_INERTIA.0 as u32) != 0 {
                println!("    - Inertia");
            }

            self.initialized = true;
            Ok(())
        }
    }

    fn apply_effect(&mut self, effect: &Effect) -> FFBResult<Vec<String>> {
        if !self.initialized || self.haptic.is_null() {
            return Err(FFBError::DeviceError("Device not initialized".to_string()));
        }

        // Clear any pending captured packets before applying effect
        let _ = self.usb_monitor.get_packets();

        // Stop previous effect
        if let Some(id) = self.current_effect_id.take() {
            unsafe {
                SDL_StopHapticEffect(self.haptic, id);
                SDL_DestroyHapticEffect(self.haptic, id);
            }
        }

        let sdl_effect = match effect {
            Effect::Constant { params, force } => self.create_constant_effect(params, force),
            Effect::Periodic { params, effect } => self.create_periodic_effect(params, effect),
            Effect::Ramp { params, effect } => self.create_ramp_effect(params, effect),
            Effect::Condition { params, effect } => self.create_condition_effect(params, effect),
        };

        unsafe {
            let effect_id = SDL_CreateHapticEffect(self.haptic, &sdl_effect);
            if effect_id.0 < 0 {
                return Err(FFBError::EffectCreationFailed(Self::get_sdl_error()));
            }

            if !SDL_RunHapticEffect(self.haptic, effect_id, 1) {
                SDL_DestroyHapticEffect(self.haptic, effect_id);
                return Err(FFBError::EffectPlaybackFailed(Self::get_sdl_error()));
            }

            self.current_effect_id = Some(effect_id);
        }

        // Wait for effect duration to allow USB capture
        let duration = effect.duration();
        if duration > 0 {
            thread::sleep(Duration::from_millis(duration as u64));
        }

        // Capture USB packets that were generated during effect playback
        let packets = self.usb_monitor.get_packets();
        let captured_packets = packets
            .iter()
            .filter(|p| UsbMonitor::is_ffb_command(p))
            .map(|p| format_hex(&p.data))
            .collect();

        Ok(captured_packets)
    }

    fn stop_all_effects(&mut self) -> FFBResult<()> {
        if self.haptic.is_null() {
            return Ok(());
        }

        if let Some(id) = self.current_effect_id.take() {
            unsafe {
                SDL_StopHapticEffect(self.haptic, id);
                SDL_DestroyHapticEffect(self.haptic, id);
            }
        }

        unsafe {
            SDL_StopHapticEffects(self.haptic);
        }

        Ok(())
    }

    fn shutdown(&mut self) -> FFBResult<()> {
        self.stop_all_effects()?;

        // Stop USB capture
        self.usb_monitor.stop_capture();

        if !self.haptic.is_null() {
            unsafe {
                SDL_CloseHaptic(self.haptic);
            }
            self.haptic = ptr::null_mut();
        }

        unsafe {
            SDL_Quit();
        }

        self.initialized = false;
        Ok(())
    }
    
    fn name(&self) -> &str {
        "SDL"
    }
    
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl Drop for SdlDriver {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

// Ensure Send + Sync for threading safety
unsafe impl Send for SdlDriver {}
unsafe impl Sync for SdlDriver {}
