//! Every integration test of this crate, as modules of one test binary: one binary links and is
//! launched once, where a binary per file would link and start once each.

mod anthropic;
mod catalog;
mod gemini;
mod openai;
mod prepare;
mod privacy;
mod record;
mod replay;
mod wire;
