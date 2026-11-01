//! Every integration test of this crate, as modules of one test binary: one binary links and is
//! launched once, where a binary per file would link and start once each.

mod aggregation_run;
mod boundary_types;
mod cache;
mod checklist;
mod config_schema;
mod contract_calls;
mod contract_fixtures;
mod contract_locations;
mod contract_schemas;
mod description;
mod image_reference;
mod images_and_frames;
mod journal_artifacts;
mod ledger;
mod obligations_merge;
mod observer;
mod repair_retry;
mod report;
mod resolve_render;
mod run_json;
mod submission_rules;
mod terminal_states;
mod tool_schemas;
mod tool_turns;
