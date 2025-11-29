pub mod audio;
pub mod analysis;
pub mod player;
pub mod export;
pub mod i18n;
pub mod gui; 

#[derive(Clone, Debug)]
pub struct LoopPoints {
    pub start_sample: usize,
    pub end_sample: usize,
    pub confidence: f32,
}