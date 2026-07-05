use crate::types::AgentPhase;

pub const PROFILE_NAME: &str = "brief";
pub const DEFAULT_MODEL: &str = "internal-balanced";

pub fn supports_phase(phase: AgentPhase) -> bool {
    matches!(phase, AgentPhase::Brief)
}
