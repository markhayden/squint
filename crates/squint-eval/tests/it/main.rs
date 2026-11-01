//! Every integration test of this crate, as modules of one test binary: one binary links and is
//! launched once, where a binary per file would link and start once each.

mod admissions;
mod arms;
mod canary;
mod common;
mod descriptor;
mod estimate;
mod eval_ledger;
mod gate;
mod matrix;
mod metrics;
mod runner;
mod sets;
mod stats;
mod summary;
