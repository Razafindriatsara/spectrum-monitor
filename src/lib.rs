//! Signalverarbeitung des Spectrum Monitors: IQ-Quellen, Spektrum, AIS und
//! Signaldetektion.
//! Der Server (`main.rs`) baut darauf auf.

pub mod ais;
pub mod detect;
pub mod dsp;
pub mod signals;
pub mod source;
