//! Signalverarbeitung des Spectrum Monitors: IQ-Quellen, Spektrum, AIS,
//! Signaldetektion und Modulationsklassifikation. Der Server (`main.rs`) und
//! der Trainer (`src/bin/train`) bauen darauf auf.

pub mod ais;
pub mod classify;
pub mod detect;
pub mod dsp;
pub mod signals;
pub mod source;
