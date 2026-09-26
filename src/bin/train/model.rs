//! Das Netz: vier Faltungsschichten mit BatchNorm und Schrittweite 2, dann
//! Mittelwert und Maximum über die Zeit, zwei vollständig verbundene Schichten.
//! Das Pooling macht das Netz unabhängig davon, wo im Fenster ein Merkmal
//! auftritt: Der Mittelwert erfasst Verteilungen, etwa wie oft die Frequenz
//! springt; das Maximum, ob überhaupt etwas Auffälliges vorkommt, etwa ein
//! Seitenband im Spektrum.

use candle_core::{D, Result, Tensor};
use candle_nn::{
    BatchNorm, BatchNormConfig, Conv1d, Conv1dConfig, Linear, Module, ModuleT, VarBuilder, batch_norm, conv1d_no_bias,
    linear,
};
use spectrum_monitor::classify::{CHANNELS, Class};

/// (Eingangskanäle, Ausgangskanäle, Kernbreite) je Faltungsschicht.
pub const CONVS: [(usize, usize, usize); 4] = [(CHANNELS, 32, 7), (32, 48, 5), (48, 64, 5), (64, 64, 3)];
pub const STRIDE: usize = 2;
pub const HIDDEN: usize = 64;

pub struct Net {
    convs: Vec<(Conv1d, BatchNorm)>,
    fc1: Linear,
    fc2: Linear,
}

/// Ein Gewichtstensor für den Export: Name, Form, Werte.
pub type Weight = (String, Vec<usize>, Vec<f32>);

impl Net {
    pub fn new(vb: VarBuilder) -> Result<Self> {
        let convs = CONVS
            .iter()
            .enumerate()
            .map(|(i, &(cin, cout, k))| {
                let cfg = Conv1dConfig { padding: k / 2, stride: STRIDE, ..Default::default() };
                // BatchNorm ersetzt den Bias der Faltung.
                let conv = conv1d_no_bias(cin, cout, k, cfg, vb.pp(format!("conv{i}")))?;
                let bn = batch_norm(cout, BatchNormConfig::default(), vb.pp(format!("bn{i}")))?;
                Ok((conv, bn))
            })
            .collect::<Result<_>>()?;
        let pooled = 2 * CONVS[CONVS.len() - 1].1;
        Ok(Self {
            convs,
            fc1: linear(pooled, HIDDEN, vb.pp("fc1"))?,
            fc2: linear(HIDDEN, Class::ALL.len(), vb.pp("fc2"))?,
        })
    }

    /// Eingang `[N, Kanäle, Fenster]`, Ausgang Logits `[N, Klassen]`. Im
    /// Training normiert BatchNorm mit den Statistiken des Stapels und führt
    /// die laufenden Mittelwerte nach, sonst nutzt sie diese.
    pub fn forward(&self, x: &Tensor, train: bool) -> Result<Tensor> {
        let mut x = x.clone();
        for (conv, bn) in &self.convs {
            x = bn.forward_t(&conv.forward(&x)?, train)?.relu()?;
        }
        let pooled = Tensor::cat(&[x.mean(D::Minus1)?, x.max(D::Minus1)?], 1)?;
        self.fc2.forward(&self.fc1.forward(&pooled)?.relu()?)
    }

    /// Gewichte für die Inferenz. BatchNorm ist in die Faltungen eingerechnet:
    /// Gewicht mal γ/√(σ²+ε), Bias β − μ·γ/√(σ²+ε).
    pub fn export_weights(&self) -> Result<Vec<Weight>> {
        let mut out = Vec::new();
        for (i, (conv, bn)) in self.convs.iter().enumerate() {
            let (gamma, beta) = bn.weight_and_bias().expect("BatchNorm mit Parametern");
            let scale = (gamma / (bn.running_var() + bn.eps())?.sqrt()?)?;
            let weight = conv.weight().broadcast_mul(&scale.reshape(((), 1, 1))?)?;
            let bias = (beta - (bn.running_mean() * &scale)?)?;
            out.push(weight_of(format!("conv{i}.weight"), &weight)?);
            out.push(weight_of(format!("conv{i}.bias"), &bias)?);
        }
        for (name, layer) in [("fc1", &self.fc1), ("fc2", &self.fc2)] {
            out.push(weight_of(format!("{name}.weight"), layer.weight())?);
            out.push(weight_of(format!("{name}.bias"), layer.bias().expect("Linear mit Bias"))?);
        }
        Ok(out)
    }
}

fn weight_of(name: String, t: &Tensor) -> Result<Weight> {
    Ok((name, t.dims().to_vec(), t.flatten_all()?.to_vec1()?))
}
