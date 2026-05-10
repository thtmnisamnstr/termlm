use half::f16;

pub fn normalize_to_f16(vec: &[f32]) -> Vec<f16> {
    let norm = vec.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    vec.iter().map(|v| f16::from_f32(*v / norm)).collect()
}

pub fn f16_dot(a: &[f16], b: &[f16]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.to_f32() * y.to_f32())
        .sum::<f32>()
}
