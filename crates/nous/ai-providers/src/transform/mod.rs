//! Message-level transforms that sit **between** a [`Context`](crate::types::Context) and a
//! provider's wire format.
//!
//! Today that is one thing: [`system`] folds a context into the transcript a provider can
//! actually send (one leading system message, no system messages in between). Upstream keeps
//! this in `packages/ai/src/utils/transcript.ts`; we ported only the collapse half — see the
//! module docs of [`system`] for what was deliberately left on the other side.

pub mod system;
