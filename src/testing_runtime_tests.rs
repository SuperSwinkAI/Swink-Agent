//! Tests for `testing`.
#![cfg(test)]

use super::{TestGpu, TestOs, TestRuntime, TestRuntimeRequirements, evaluate_test_runtime};

#[test]
fn runtime_rejects_os_mismatch() {
    let runtime = TestRuntime {
        os: TestOs::Linux,
        arch: "x86_64",
        has_any_gpu: false,
        has_nvidia_gpu: false,
        has_apple_metal_gpu: false,
    };

    let reason = evaluate_test_runtime(
        &runtime,
        TestRuntimeRequirements::new().with_os(TestOs::MacOs),
    )
    .expect_err("linux host should not satisfy macOS-only requirement");

    assert!(reason.contains("requires macOS"));
}

#[test]
fn runtime_rejects_missing_gpu() {
    let runtime = TestRuntime {
        os: TestOs::Linux,
        arch: "x86_64",
        has_any_gpu: false,
        has_nvidia_gpu: false,
        has_apple_metal_gpu: false,
    };

    let reason = evaluate_test_runtime(
        &runtime,
        TestRuntimeRequirements::new().with_gpu(TestGpu::Any),
    )
    .expect_err("gpu-less host should not satisfy gpu requirement");

    assert!(reason.contains("requires a detected GPU"));
}

#[test]
fn runtime_accepts_nvidia_gpu_requirement() {
    let runtime = TestRuntime {
        os: TestOs::Linux,
        arch: "x86_64",
        has_any_gpu: true,
        has_nvidia_gpu: true,
        has_apple_metal_gpu: false,
    };

    let result = evaluate_test_runtime(
        &runtime,
        TestRuntimeRequirements::new().with_gpu(TestGpu::Nvidia),
    );

    assert!(result.is_ok());
}
