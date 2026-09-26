//! Signalverarbeitung des Spectrum Monitors: IQ-Quellen, Spektrum, AIS,
//! Signaldetektion und Modulationsklassifikation.
//! Der Server (`main.rs`) baut darauf auf.

pub mod ais;
pub mod classify;
pub mod detect;
pub mod dsp;
pub mod signals;
pub mod source;
