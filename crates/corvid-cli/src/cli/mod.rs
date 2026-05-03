//! Clap argument tree for the `corvid` CLI — slice 20j-A1.
//!
//! This module collects the per-command-group clap
//! `Subcommand` / `ValueEnum` definitions that previously lived
//! inline in `main.rs`. Each per-group submodule owns its own
//! arg tree so adding a new subcommand to `corvid jobs *` (or
//! any other group) only touches one focused file.
//!
//! Subsequent commits 20j-A1 #3 and #4 add `package`, `observe`,
//! and `eval` per-group submodules; the connector / auth /
//! approvals / contract / claim / abi / approver / capsule /
//! bench / trace / receipt / bundle / deploy / upgrade arg trees
//! follow as the dispatch tree is extracted.

pub mod bench;
pub mod connectors;
pub mod contract;
pub mod jobs;
pub mod migrate;
pub mod observe;
pub mod package;
pub mod root;

#[allow(unused_imports)]
pub use bench::*;
#[allow(unused_imports)]
pub use connectors::*;
#[allow(unused_imports)]
pub use contract::*;
#[allow(unused_imports)]
pub use jobs::*;
#[allow(unused_imports)]
pub use migrate::*;
#[allow(unused_imports)]
pub use observe::*;
#[allow(unused_imports)]
pub use package::*;
#[allow(unused_imports)]
pub use root::*;
