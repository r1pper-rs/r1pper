/// A source result that can be passed directly to `r1pper download`.
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub plugin_id: String,
    pub title: String,
    pub url: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub size: Option<u64>,
    pub artwork_url: Option<String>,
}
