use thiserror::Error;

#[derive(Error, Debug)]
pub enum FFBError {
    #[error("Device not found")]
    DeviceNotFound,
    
    #[error("Failed to initialize device: {0}")]
    InitializationFailed(String),
    
    #[error("Failed to create effect: {0}")]
    EffectCreationFailed(String),
    
    #[error("Failed to play effect: {0}")]
    EffectPlaybackFailed(String),
    
    #[error("Failed to stop effect: {0}")]
    EffectStopFailed(String),
    
    #[error("Device error: {0}")]
    DeviceError(String),
    
    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),
}

pub type FFBResult<T> = Result<T, FFBError>;
