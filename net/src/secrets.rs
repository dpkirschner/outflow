//! Re-export of [`outflow_core::secrets`]: the helpers live in `core` so the
//! CLI's `encryption` build can read a 0600 key file without pulling in the
//! network stack. Kept here so existing `outflow_net::secrets` paths work.

pub use outflow_core::secrets::*;
