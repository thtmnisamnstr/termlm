pub mod cheatsheet;
pub mod chunk;
pub mod embed;
pub mod extract;
pub mod lexical;
pub mod lookup;
pub mod retrieve;
pub mod scan;
pub mod store;
pub mod watch;

pub use chunk::{Chunk, Chunker};
pub use lookup::lookup_command_docs;
pub use retrieve::{HybridRetriever, RetrievalQuery, RetrievalResult};
pub use scan::{
    BinaryEntry, DiscoverBinariesResult, discover_binaries, discover_binaries_with_stats,
};
pub use store::{IndexManifest, IndexStore, LayoutWriteArtifacts};
