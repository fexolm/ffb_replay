use crate::{effects::Effect, error::FFBResult};
use std::any::Any;

/// Trait for Force Feedback device drivers
pub trait FfbDriver {
    /// Initialize the device
    fn initialize(&mut self) -> FFBResult<()>;
    
    /// Apply (create and start) an effect
    /// Returns captured/generated command packets as hex strings
    /// For real drivers (SDL), this waits for effect duration and captures USB traffic
    /// For simulation drivers (Simagic), this returns generated reports immediately
    fn apply_effect(&mut self, effect: &Effect) -> FFBResult<Vec<String>>;
    
    /// Stop all effects
    fn stop_all_effects(&mut self) -> FFBResult<()>;
    
    /// Shutdown the device and release resources
    fn shutdown(&mut self) -> FFBResult<()>;
    
    /// Get the driver name for logging
    fn name(&self) -> &str;
    
    /// Downcast to Any for type-specific operations
    fn as_any(&self) -> &dyn Any;
}
