//! The restricted API surface every card hook acts through — narrow on
//! purpose, so a card can't do anything the context doesn't expose, and so
//! card behavior is testable in isolation with a mock context rather than
//! a full running game.

mod audit_context;
mod interaction_context;
mod play_context;

pub use audit_context::AuditContext;
pub use interaction_context::InteractionContext;
pub use play_context::{CaptureMode, CaptureOutcome, MovementProposal, PlayContext};
