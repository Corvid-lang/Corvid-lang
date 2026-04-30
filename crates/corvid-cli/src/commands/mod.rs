//! Per-command dispatch behaviours that the top-level `Command`
//! enum's match arms call into. Slice 20j-A1 commit 11a kicked
//! off the extraction; subsequent commits (11b-d) move the
//! observe/eval/test/misc dispatch arms here, leaving main.rs
//! as a thin entry-point shell.

pub mod eval;
pub mod jobs;
pub mod misc;
pub mod test;

#[allow(unused_imports)]
pub use eval::*;
#[allow(unused_imports)]
pub use jobs::*;
#[allow(unused_imports)]
pub use misc::*;
#[allow(unused_imports)]
pub use test::*;
