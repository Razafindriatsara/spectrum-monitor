//! Schreibt das trainierte Netz als ONNX-Modell. Kein Rust-Framework
//! exportiert ONNX, also wird der Graph hier direkt aus den Protobuf-Typen
//! gebaut, die tract-onnx mitbringt: Conv, Relu, GlobalAveragePool,
//! GlobalMaxPool, Concat, Flatten, Gemm und Softmax, dazu die Gewichte als
//! Initialisierer. BatchNorm steckt schon in den Faltungsgewichten.

use super::model::{CONVS, STRIDE, Weight};
use prost::Message;
use spectrum_monitor::classify::{CHANNELS, Class, WINDOW};
use tract_onnx::pb::attribute_proto::AttributeType;
use tract_onnx::pb::tensor_proto::DataType;
use tract_onnx::pb::tensor_shape_proto::{Dimension, dimension};
use tract_onnx::pb::{
    AttributeProto, GraphProto, ModelProto, NodeProto, OperatorSetIdProto, StringStringEntryProto, TensorProto,
    TensorShapeProto, TypeProto, ValueInfoProto, type_proto,
};

pub const INPUT: &str = "features";
pub const OUTPUT: &str = "probabilities";

/// Baut das Modell aus den Gewichten des Netzes (`conv0.weight`, … `fc2.bias`).
pub fn export(weights: &[Weight], metadata: &[(&str, String)]) -> Vec<u8> {
    let initializer = weights
        .iter()
        .map(|(name, dims, data)| TensorProto {
            name: name.clone(),
            dims: dims.iter().map(|&d| d as i64).collect(),
            data_type: DataType::Float as i32,
            raw_data: data.iter().flat_map(|v| v.to_le_bytes()).collect(),
            ..Default::default()
        })
        .collect();

    let mut node = Vec::new();
    let mut prev = INPUT.to_string();
    for (i, &(_, _, k)) in CONVS.iter().enumerate() {
        let (conv, relu) = (format!("conv{i}.out"), format!("relu{i}.out"));
        let pad = (k / 2) as i64;
        node.push(op(
            "Conv",
            &[&prev, &format!("conv{i}.weight"), &format!("conv{i}.bias")],
            &conv,
            vec![
                ints("kernel_shape", &[k as i64]),
                ints("pads", &[pad, pad]),
                ints("strides", &[STRIDE as i64]),
                ints("dilations", &[1]),
                int("group", 1),
            ],
        ));
        node.push(op("Relu", &[&conv], &relu, vec![]));
        prev = relu;
    }
    node.push(op("GlobalAveragePool", &[&prev], "mean.out", vec![]));
    node.push(op("GlobalMaxPool", &[&prev], "max.out", vec![]));
    node.push(op("Concat", &["mean.out", "max.out"], "pool.out", vec![int("axis", 1)]));
    node.push(op("Flatten", &["pool.out"], "flat.out", vec![int("axis", 1)]));
    node.push(op("Gemm", &["flat.out", "fc1.weight", "fc1.bias"], "fc1.out", vec![int("transB", 1)]));
    node.push(op("Relu", &["fc1.out"], "fc1.relu", vec![]));
    node.push(op("Gemm", &["fc1.relu", "fc2.weight", "fc2.bias"], "logits", vec![int("transB", 1)]));
    node.push(op("Softmax", &["logits"], OUTPUT, vec![int("axis", 1)]));

    let graph = GraphProto {
        name: "modulation".into(),
        node,
        initializer,
        input: vec![value(INPUT, &[None, Some(CHANNELS), Some(WINDOW)])],
        output: vec![value(OUTPUT, &[None, Some(Class::ALL.len())])],
        ..Default::default()
    };
    let model = ModelProto {
        ir_version: 7,
        opset_import: vec![OperatorSetIdProto { domain: String::new(), version: 13 }],
        producer_name: "spectrum-monitor train".into(),
        producer_version: env!("CARGO_PKG_VERSION").into(),
        doc_string: "Modulationsklassifikation für Schmalbandsignale bei 48 kHz".into(),
        graph: Some(graph),
        metadata_props: metadata
            .iter()
            .map(|(k, v)| StringStringEntryProto { key: k.to_string(), value: v.clone() })
            .collect(),
        ..Default::default()
    };
    model.encode_to_vec()
}

fn op(op_type: &str, inputs: &[&str], output: &str, attribute: Vec<AttributeProto>) -> NodeProto {
    NodeProto {
        op_type: op_type.into(),
        name: output.into(),
        input: inputs.iter().map(|s| s.to_string()).collect(),
        output: vec![output.into()],
        attribute,
        ..Default::default()
    }
}

fn ints(name: &str, v: &[i64]) -> AttributeProto {
    AttributeProto { name: name.into(), r#type: AttributeType::Ints as i32, ints: v.to_vec(), ..Default::default() }
}

fn int(name: &str, v: i64) -> AttributeProto {
    AttributeProto { name: name.into(), r#type: AttributeType::Int as i32, i: v, ..Default::default() }
}

/// Tensorbeschreibung; `None` ist die Stapelgröße, die offen bleibt.
fn value(name: &str, dims: &[Option<usize>]) -> ValueInfoProto {
    let dim = dims
        .iter()
        .map(|d| Dimension {
            value: Some(match d {
                Some(n) => dimension::Value::DimValue(*n as i64),
                None => dimension::Value::DimParam("N".into()),
            }),
            ..Default::default()
        })
        .collect();
    ValueInfoProto {
        name: name.into(),
        r#type: Some(TypeProto {
            value: Some(type_proto::Value::TensorType(type_proto::Tensor {
                elem_type: DataType::Float as i32,
                shape: Some(TensorShapeProto { dim }),
            })),
            ..Default::default()
        }),
        ..Default::default()
    }
}
