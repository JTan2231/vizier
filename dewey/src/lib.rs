use crate::cache::EmbeddingCache;
use crate::config::get_data_dir;
use crate::dbio::{Embedding, EmbeddingSource, BLOCK_SIZE};
use crate::hnsw::{Query, HNSW};

pub mod cache;
pub mod config;
pub mod dbio;
pub mod hnsw;
pub mod serialization;

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        if cfg!(feature = "verbose") {
            println!("{}", format!("{} [INFO]: {}", chrono::Local::now(), format!($($arg)*)));
        }
    };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
            println!("{}", format!("{} [INFO]: {}", chrono::Local::now(), format!($($arg)*)));
    };
}

// TODO: Absolute nightmare crate. This needs to be rewritten as a SQLite plugin or something.
//       Dear God.

/// This is an old in-memory programmatic API
/// This should really be deprecated/refactored
pub struct Dewey {
    index: hnsw::HNSW,
    cache: EmbeddingCache,
}

impl Dewey {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        crate::config::setup()?;

        // We're rebuilding the index from the blocks for now because it's assumed that the number
        // of messages will be small enough to warrant this
        // More than a few blocks, however. will probably warrant some sort of process for building
        // these in the background
        //
        // TODO: Figure something out to keep the index fresh without compromising performance
        Ok(Self {
            index: HNSW::new(true)?,
            cache: EmbeddingCache::new((20 * BLOCK_SIZE) as u32)?,
        })
    }

    // TODO: better define how filters should be passed
    pub fn query(
        &mut self,
        embedding: Embedding,
        k: usize,
    ) -> Result<Vec<EmbeddingSource>, std::io::Error> {
        // TODO:
        let filters = vec![];

        let query = Query { embedding, filters };

        Ok(self
            .index
            .query(&mut self.cache, &query, k, 200)
            .iter()
            .map(|p| p.0.source_file.clone())
            .collect())
    }

    /// Add a new embedding to the system from the given file
    ///
    /// This updates both:
    /// - The embedding store in the OS file system
    /// - The in-memory HNSW index
    ///
    /// Alongside related metadata + other housekeeping files in the OS filesystem:
    /// - Embedding store directory
    /// - HNSW index file
    pub fn add_embedding(&mut self, mut embedding: Embedding) -> Result<(), std::io::Error> {
        match dbio::add_new_embedding(&mut embedding) {
            Ok(_) => {}
            Err(e) => {
                error!("error adding embedding to store: {}", e);
                return Err(e);
            }
        };

        info!("Created embedding with id: {}", embedding.id);
        info!("Finished writing embedding to file system");

        self.cache.refresh_directory()?;
        info!("Refreshed cache directory");

        match self.index.insert(&mut self.cache, &embedding) {
            Ok(_) => {}
            Err(e) => {
                error!("Error adding embedding to index: {}", e);
                return Err(e);
            }
        };

        match self
            .index
            .serialize(&get_data_dir().join("index").to_str().unwrap().to_string())
        {
            Ok(_) => {}
            Err(e) => {
                error!("error serializing index: {}", e);
                return Err(e);
            }
        };

        info!("Updated index with new embedding");

        Ok(())
    }
}
