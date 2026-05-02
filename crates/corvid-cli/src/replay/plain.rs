//! Plain-replay stub.
//!
//! `corvid replay <trace>` (no `--model`, no `--mutate`) lands
//! here. Today the stub emits a "not yet available" diagnostic
//! pointing at the Phase 21 slices that ship the actual replay
//! runtime; once `21-C-replay-interp` / `21-C-replay-native`
//! land, this branch invokes the replay runtime.

use anyhow::Result;

use super::EXIT_NOT_IMPLEMENTED;

pub(super) fn plain_replay_stub() -> Result<u8> {
    eprintln!();
    eprintln!(
        "note: `corvid replay` is not yet available. Replay-runtime support \
         ships in Phase 21 slice 21-C-replay-interp (interpreter tier) and \
         21-C-replay-native (native tier). Trace load + schema validation \
         succeeded above; once the runtime slices land, this command will \
         re-execute the program with recorded responses substituted for live \
         calls."
    );
    Ok(EXIT_NOT_IMPLEMENTED)
}
