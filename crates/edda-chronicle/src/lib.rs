pub mod anchor;
pub mod attention;
pub mod classify;
pub mod extract;
pub mod relate;
pub mod state;
pub mod synthesize;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecapOutput {
    pub net_result: String,
    pub needs_you: String,
    pub decision_context: String,
    pub relations: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecapOptions {
    pub query: Option<String>,
    pub project: Option<String>,
    pub week: bool,
    pub since: Option<String>,
    pub all: bool,
    pub json: bool,
}

pub use anchor::{resolve_anchor, Anchor, ResolvedAnchor};
pub use attention::{get_attention_items, AttentionItem};
pub use classify::{classify_session, SessionType};
pub use extract::{extract_key_turns, KeyTurn};
pub use relate::{find_related_content, RelatedContent};
pub use state::{load_state, save_state, LastRecap, RecapState};
pub use synthesize::{synthesize_recap, SynthesisInput, TurnContent};
