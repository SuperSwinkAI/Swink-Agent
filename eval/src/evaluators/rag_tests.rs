//! Tests for `rag`.
#![cfg(test)]

use super::*;

#[test]
fn cosine_similarity_of_identical_vectors_is_one() {
    let a = vec![1.0_f32, 0.0, 0.0];
    assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-9);
}

#[test]
fn cosine_similarity_of_opposite_vectors_is_minus_one() {
    let a = vec![1.0_f32, 0.0];
    let b = vec![-1.0_f32, 0.0];
    assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-9);
}

#[test]
fn cosine_similarity_orthogonal_vectors_is_zero() {
    let a = vec![1.0_f32, 0.0];
    let b = vec![0.0_f32, 1.0];
    assert!(cosine_similarity(&a, &b).abs() < 1e-9);
}

#[test]
fn cosine_similarity_mismatched_dims_is_zero() {
    let a = vec![1.0_f32, 0.0];
    let b = vec![1.0_f32];
    assert!(cosine_similarity(&a, &b).abs() < 1e-9);
}

#[test]
fn cosine_similarity_empty_vectors_is_zero() {
    let a: Vec<f32> = vec![];
    let b: Vec<f32> = vec![];
    assert!(cosine_similarity(&a, &b).abs() < 1e-9);
}
