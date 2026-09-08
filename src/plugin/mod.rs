//! Extism WASM source-plugin discovery and resolution.

mod manifest;
mod protocol;

pub use manifest::{PluginDescriptor, PluginRegistry, ResolvedSource};
pub use protocol::{
    MediaCandidate, PLUGIN_API_VERSION, ResolveRequest, ResolveResponse, ResolvedItem,
    SearchRequest, SearchResponse, SourceMetadata,
};
