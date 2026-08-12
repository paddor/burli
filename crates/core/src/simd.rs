#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuTier {
    Scalar,
    Simd,
}

pub fn cpu_tier() -> CpuTier {
    CpuTier::Scalar
}
