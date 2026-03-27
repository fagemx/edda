mod trigger;
mod types;
mod writer;

pub use trigger::evaluate_trigger;
pub use types::{IngestionRecord, SourceLayer, SourceRef, TriggerResult};
pub use writer::write_ingestion_record;
