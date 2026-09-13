//! pack: ingest (stream, resolve, normalize — runs in the edge) and generate (send-set,
//! pack writer — runs in the DO). CONTRACTS.md 1.4, 2.4, 9.

pub mod generate;
pub mod ingest;
pub mod run;
