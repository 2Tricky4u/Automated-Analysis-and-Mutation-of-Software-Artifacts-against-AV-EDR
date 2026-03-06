//! Artifact metadata indexing to Elasticsearch.
//!
//! Stores build-time metadata (compiler flags, mutation recipe, hashes) in the
//! `artifacts-YYYY.MM` index so each binary can be traced back to its origin.

use elasticsearch::params::Refresh;
use elasticsearch::{Elasticsearch, IndexParts};
use serde_json::Value;

/// Index artifact build metadata to the `artifacts-YYYY.MM` index.
///
/// # Errors
///
/// Returns an error if the Elasticsearch index request fails or the response
/// indicates a non-success status code.
pub async fn index_artifact(es: &Elasticsearch, doc: Value) -> anyhow::Result<()> {
    let index = super::helpers::es_index_name("artifacts");
    let response = es
        .index(IndexParts::Index(&index))
        .body(doc)
        .refresh(Refresh::WaitFor)
        .send()
        .await?;
    super::helpers::check_index_response(response, "artifact", "").await
}
