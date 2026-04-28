//! Eval-driven self-improvement loop for swink-agent prompts and tool schemas.
//!
//! Runs a closed-loop optimization cycle: baseline evaluation → diagnosis →
//! mutation → re-evaluation → acceptance gating → versioned persistence.
#![forbid(unsafe_code)]

pub mod config;
pub mod diagnose;
pub mod evaluate;
pub mod gate;
pub mod mutate;
pub mod persist;
pub mod runner;
pub mod strategies;
pub mod types;

// Configuration
pub use config::{CycleBudget, OptimizationConfig, OptimizationTarget, PromptSection};

// Diagnosis
pub use diagnose::{CaseFailure, Diagnoser, TargetComponent, WeakPoint};

// Mutation traits and types
pub use mutate::{Candidate, MutationContext, MutationError, MutationStrategy, deduplicate};

// Mutation strategies
pub use strategies::{Ablation, LlmGuided, TemplateBased};

// Evaluation & gating
pub use evaluate::CandidateResult;
pub use gate::{AcceptanceGate, AcceptanceResult, AcceptanceVerdict};

// Core runner
pub use runner::{EvolutionRunner, EvolveError};

// Results
pub use types::{BaselineSnapshot, CycleResult, CycleStatus};

// Persistence
pub use persist::{CyclePersister, ManifestEntry};
