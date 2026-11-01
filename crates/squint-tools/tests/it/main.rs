//! Every integration test of this crate, as modules of one test binary: one binary links and is
//! launched once, where a binary per file would link and start once each.

mod align;
mod align_tool;
mod builtin;
mod compare;
mod mcp_run;
mod planner;
mod provisioning;
mod python_pack;
