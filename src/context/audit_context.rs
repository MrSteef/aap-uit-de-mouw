//! `AuditContext`: the restricted API surface a card's audit-resolution
//! hooks act through.

use crate::pawn::PawnId;
use crate::player::PlayerId;

/// The context a card's `on_audited_as_played`/`on_audited_as_claimed`
/// hooks act through.
pub struct AuditContext<'a> {
    pub auditor: PlayerId,
    pub auditee: PlayerId,
    pub target_pawn: PawnId,
    forfeit_auditor_turn: &'a mut bool,
}

impl<'a> AuditContext<'a> {
    /// Marks that the auditor's turn is forfeited, independent of whether
    /// the audit itself caught a lie.
    pub fn forfeit_auditor_turn(&mut self) {
        *self.forfeit_auditor_turn = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forfeit_auditor_turn_sets_the_shared_flag() {
        let mut forfeit = false;
        let mut ctx = AuditContext {
            auditor: PlayerId(0),
            auditee: PlayerId(1),
            target_pawn: PawnId(0),
            forfeit_auditor_turn: &mut forfeit,
        };
        ctx.forfeit_auditor_turn();
        assert!(forfeit);
    }
}
