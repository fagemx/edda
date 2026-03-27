mod suggestion;
mod trigger;
mod types;
mod writer;

pub use suggestion::SuggestionQueue;
pub use trigger::evaluate_trigger;
pub use types::{
    IngestionRecord, SourceLayer, SourceRef, Suggestion, SuggestionStatus, TriggerResult,
    TriggerType,
};
pub use writer::write_ingestion_record;
