use std::collections::{HashMap, HashSet};
use std::io::Write;

use dewey_macros::Serialize;

use crate::cache::EmbeddingCache;
use crate::config::{get_data_dir, get_local_dir};
use crate::hnsw::{normalize, HNSW};
use crate::serialization::Serialize;
use crate::{error, info};

// TODO: this could probably be a config parameter
pub const BLOCK_SIZE: usize = 1024;

pub const EMBED_DIM: usize = 1536;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct EmbeddingSource {
    pub filepath: String,
    pub subset: Option<(u64, u64)>,
}

impl EmbeddingSource {
    pub fn new() -> Self {
        EmbeddingSource {
            filepath: String::new(),
            subset: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Embedding {
    pub id: u64,
    pub source_file: EmbeddingSource,
    pub data: [f32; EMBED_DIM],
}

#[derive(Debug, Serialize)]
pub struct EmbeddingBlock {
    block: u64,
    pub embeddings: Vec<Embedding>,
}

impl EmbeddingBlock {
    fn to_file(&self, filename: &str) -> Result<(), std::io::Error> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(filename)?;

        let bytes = self.to_bytes();
        info!("Writing {} bytes to {}", bytes.len(), filename);
        file.write_all(&bytes)?;

        Ok(())
    }
}

struct DirectoryEntry {
    id: u32,
    filepath: String,
}

// directory for which embeddings are in which blocks
pub struct Directory {
    pub file_map: HashMap<String, u64>, // Embedding source filepath -> block number
    pub id_map: HashMap<u32, u64>,      // Embedding ID -> block number
    pub file_id_map: HashMap<String, u32>, // Embedding source filepath -> embedding ID
}

impl Directory {
    pub fn len(&self) -> usize {
        self.id_map.len()
    }
}

fn write_directory(entries: &Vec<(DirectoryEntry, u32)>) -> Result<(), std::io::Error> {
    let directory = entries
        .into_iter()
        .map(|d| format!("{} {} {}", d.0.id, d.0.filepath, d.1))
        .collect::<Vec<_>>();
    let count = directory.len();
    let directory = directory.join("\n");

    std::fs::write(
        format!("{}/directory", get_data_dir().to_str().unwrap()),
        directory,
    )?;

    info!("Wrote directory with {} entries", count);

    Ok(())
}

// NOTE: not thread safe
fn get_next_id() -> Result<u64, std::io::Error> {
    let counter_path = get_local_dir().join("id_counter");
    let contents = match std::fs::read_to_string(&counter_path) {
        Ok(c) => {
            if c.is_empty() {
                "0".to_string()
            } else {
                c
            }
        }
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => "0".to_string(),
            _ => {
                error!("error opening ID counter file: {}", e);
                return Err(e);
            }
        },
    };

    let last_id = match contents.parse::<u64>() {
        Ok(id) => id,
        Err(e) => {
            error!("error reading ID counter file: {e}");
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e));
        }
    };

    let new_id = last_id + 1;

    if let Err(e) = std::fs::write(&counter_path, new_id.to_string()) {
        error!("error writing new ID: {e}");
        return Err(e);
    }

    Ok(new_id)
}

// optimizes embedding placement in blocks based on their distance from their neighbors
pub fn reblock() -> Result<(), std::io::Error> {
    let index = match HNSW::new(false) {
        Ok(index) => index,
        Err(e) => {
            eprintln!("Error creating index: {}", e);
            eprintln!("Note: this operation requires an index to be present");
            eprintln!("Run `hnsw -s` to recreate your index");
            return Err(e);
        }
    };

    let full_graph = match index.get_last_layer() {
        Some(g) => g,
        None => {
            info!("index is empty; nothing to do.");
            return Ok(());
        }
    };

    let mut blocks = vec![Vec::new()];
    let mut i = 0;

    let mut visited = HashSet::new();
    let mut stack = Vec::new();
    stack.push(*full_graph.iter().nth(0).unwrap().0);

    while let Some(current) = stack.pop() {
        if visited.contains(&current) {
            continue;
        }

        if full_graph.len() > 10 && visited.len() % (full_graph.len() / 10) == 0 {
            info!("blocked {} nodes into {} blocks", visited.len(), i + 1);
        }

        if blocks[i].len() >= BLOCK_SIZE {
            blocks.push(Vec::new());
            i += 1;
        }

        blocks[i].push(current);
        visited.insert(current);

        let mut neighbors = full_graph.get(&current).unwrap().clone();
        neighbors.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        for (neighbor, _) in neighbors {
            if !visited.contains(&neighbor) {
                stack.push(neighbor);
            }
        }
    }

    let mut cache = EmbeddingCache::new(10 * BLOCK_SIZE as u32)?;

    // create a temp directory in $DATA_DIR to hold all the blocks
    let temp_dir = format!("{}/temp", get_data_dir().to_str().unwrap());

    if std::fs::metadata(&temp_dir).is_ok() {
        std::fs::remove_dir_all(&temp_dir)?;
    }

    std::fs::create_dir(&temp_dir)?;

    let mut directory = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        let filename = format!("{}/{}", temp_dir, i);
        let mut embeddings = Vec::new();
        for id in block {
            let embedding = cache.get(*id as u32).unwrap();

            directory.push((
                DirectoryEntry {
                    id: embedding.id as u32,
                    filepath: embedding.source_file.filepath.clone(),
                },
                i as u32,
            ));

            embeddings.push(*embedding);
        }

        let embedding_block = EmbeddingBlock {
            block: i as u64,
            embeddings,
        };

        embedding_block.to_file(&filename)?;
    }

    for entry in std::fs::read_dir(get_data_dir().clone())? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Some(filename) = path.file_name() {
                if let Some(filename) = filename.to_str() {
                    if filename.parse::<u64>().is_ok() {
                        std::fs::remove_file(path)?;
                    }
                }
            }
        }
    }

    std::fs::remove_file(format!("{}/directory", get_data_dir().to_str().unwrap()))?;

    for entry in std::fs::read_dir(temp_dir.clone())? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Some(filename) = path.file_name() {
                if let Some(filename) = filename.to_str() {
                    if filename.parse::<u64>().is_ok() {
                        std::fs::rename(
                            path.clone(),
                            format!("{}/{}", get_data_dir().to_str().unwrap(), filename),
                        )?;
                    }
                }
            }
        }
    }

    std::fs::remove_dir_all(&temp_dir)?;

    match write_directory(&directory) {
        Ok(_) => {}
        Err(e) => {
            error!("error writing directory: {}", e);
            return Err(e);
        }
    };

    Ok(())
}

pub fn read_embedding_block(block_number: u64) -> Result<EmbeddingBlock, std::io::Error> {
    let bytes = match std::fs::read(&format!(
        "{}/{}",
        get_data_dir().to_str().unwrap(),
        block_number
    )) {
        Ok(b) => b,
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => Vec::new(),
            _ => {
                error!("error reading block file {}: {}", block_number, e);
                return Err(e);
            }
        },
    };

    let (block, _) = if bytes.is_empty() {
        (
            EmbeddingBlock {
                block: 0,
                embeddings: Vec::new(),
            },
            0,
        )
    } else {
        match EmbeddingBlock::from_bytes(&bytes, 0) {
            Ok(b) => b,
            Err(e) => {
                error!("error parsing block file {}: {}", block_number, e);
                return Err(e);
            }
        }
    };

    Ok(block)
}

pub struct BlockEmbedding {
    pub block_number: u64,
    pub embedding: Box<Embedding>,
    pub source_file: String,
}

// returns boxes of the embeddings and the block files from which they were read
pub fn get_all_blocks() -> Result<Vec<BlockEmbedding>, std::io::Error> {
    let mut block_numbers = Vec::new();
    for entry in std::fs::read_dir(get_data_dir().clone())? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Some(filename) = path.file_name() {
                if let Some(filename) = filename.to_str() {
                    if filename.parse::<u64>().is_ok() {
                        block_numbers.push(filename.parse::<u64>().unwrap());
                    }
                }
            }
        }
    }

    let mut block_embeddings = Vec::new();
    for block_number in block_numbers {
        let filename = format!("{}/{}", get_data_dir().to_str().unwrap(), block_number);
        let block = read_embedding_block(block_number)?;

        for be in block
            .embeddings
            .into_iter()
            .map(|mut embedding| {
                normalize(&mut embedding);
                Box::new(embedding)
            })
            .collect::<Vec<_>>()
        {
            block_embeddings.push(BlockEmbedding {
                block_number,
                embedding: be,
                source_file: filename.clone(),
            });
        }
    }

    Ok(block_embeddings)
}

// TODO: at what point should we worry about holding this whole thing in memory?
//       it shouldn't stay like this forever
//       i think the directory should be grouped in separate files by both:
//         - layers
//       and
//         - embedding blocks
pub fn get_directory() -> Result<Directory, std::io::Error> {
    let directory =
        match std::fs::read_to_string(format!("{}/directory", get_data_dir().to_str().unwrap())) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                error!("error reading directory file: {}", e);
                return Err(e);
            }
        };

    let directory = directory
        .split("\n")
        .filter(|l| !l.is_empty())
        .map(|d| {
            let parts = d.split(" ").collect::<Vec<&str>>();
            let id = parts[0].parse::<u32>().unwrap();
            let filepath = parts[1..parts.len() - 1].join("");
            let block = parts[parts.len() - 1].parse::<u64>().unwrap();

            (id, filepath, block)
        })
        .collect::<Vec<_>>();

    // Embedding ID -> block number
    let mut id_map = HashMap::new();

    // Embedding source filepath -> block number
    let mut file_map = HashMap::new();

    // Embedding source filepath -> embedding ID
    let mut file_id_map = HashMap::new();

    for entry in directory.iter() {
        id_map.insert(entry.0, entry.2);
        file_map.insert(entry.1.clone(), entry.2);
        file_id_map.insert(entry.1.clone(), entry.0);
    }

    Ok(Directory {
        id_map,
        file_map,
        file_id_map,
    })
}

/// this adds a new embedding to the embedding store
///
/// the last block is chosen (arbitrarily) as its new home
/// the directory file is also updated with an entry for the new embedding
///
/// this _does not_ affect the HNSW index--in-memory or otherwise
/// updates to the index should take place with that struct directly
/// this function here is specifically for adding the embeddings
/// to the file system
pub fn add_new_embedding(embedding: &mut Embedding) -> Result<(), std::io::Error> {
    let last_block_number = match std::fs::read_dir(get_data_dir())
        .unwrap()
        .into_iter()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let filename = entry.file_name();
            let filename_str = filename.to_str()?;

            // Try to parse the filename as a number
            filename_str.parse::<u64>().ok()
        })
        .max()
    {
        Some(bn) => bn,
        None => 0,
    };

    let mut block = match read_embedding_block(last_block_number) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => EmbeddingBlock {
            block: 0,
            embeddings: Vec::new(),
        },
        Err(e) => {
            return Err(e);
        }
    };

    embedding.id = get_next_id()?;
    block.embeddings.push(embedding.clone());

    let filepath = format!("{}/{}", get_data_dir().to_str().unwrap(), block.block);
    block.to_file(&filepath)?;

    info!("Saved embedding to {}", filepath);

    let mut directory = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(get_data_dir().join("directory"))?;

    writeln!(
        directory,
        "\n{} {} {}",
        embedding.id, embedding.source_file.filepath, last_block_number
    )?;

    info!("Directory updated");

    Ok(())
}

/// this adds a new embedding to the embedding store
///
/// the last block is chosen (arbitrarily) as its new home
/// the directory file is also updated with an entry for the new embedding
///
/// this _does not_ affect the HNSW index--in-memory or otherwise
/// updates to the index should take place with that struct directly
/// this function here is specifically for adding the embeddings
/// to the file system
pub fn upsert_embedding(embedding: &mut Embedding) -> Result<(), Box<dyn std::error::Error>> {
    let directory = get_directory()?;

    let (block_num, embed_id) = (
        directory.file_map.get(&embedding.source_file.filepath),
        directory.file_id_map.get(&embedding.source_file.filepath),
    );

    let is_insert = block_num.is_none() || embed_id.is_none();

    let target_block_number = if is_insert {
        match std::fs::read_dir(get_data_dir())
            .unwrap()
            .into_iter()
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let filename = entry.file_name();
                let filename_str = filename.to_str()?;

                // Try to parse the filename as a number
                filename_str.parse::<u64>().ok()
            })
            .max()
        {
            Some(bn) => bn,
            None => 0,
        }
    } else {
        *block_num.unwrap()
    };

    let mut block = match read_embedding_block(target_block_number) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => EmbeddingBlock {
            block: 0,
            embeddings: Vec::new(),
        },
        Err(e) => {
            return Err(Box::new(e));
        }
    };

    if is_insert {
        embedding.id = get_next_id()?;
        block.embeddings.push(embedding.clone());
    } else {
        let block_embed = block
            .embeddings
            .iter_mut()
            .find(|e| e.source_file == embedding.source_file)
            .ok_or(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Embedding not found",
            ))?;

        block_embed.data = embedding.data;
    }

    let filepath = format!("{}/{}", get_data_dir().to_str().unwrap(), block.block);
    block.to_file(&filepath)?;

    info!("Saved embedding to {}", filepath);

    // the embedding will only have moved if it's being inserted--updates will be in place
    if is_insert {
        let mut directory = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(get_data_dir().join("directory"))?;

        writeln!(
            directory,
            "\n{} {} {}",
            embedding.id, embedding.source_file.filepath, target_block_number
        )?;

        info!("Directory updated");
    }

    Ok(())
}

/// The same as `add_new_embedding`, except this is for sets of embeddings
pub fn add_new_embeddings(embeddings: &mut Vec<Embedding>) -> Result<(), std::io::Error> {
    let last_block_number = match std::fs::read_dir(get_data_dir())
        .unwrap()
        .into_iter()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let filename = entry.file_name();
            let filename_str = filename.to_str()?;

            // Try to parse the filename as a number
            filename_str.parse::<u64>().ok()
        })
        .max()
    {
        Some(bn) => bn,
        None => 0,
    };

    let mut block = match read_embedding_block(last_block_number) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => EmbeddingBlock {
            block: 0,
            embeddings: Vec::new(),
        },
        Err(e) => {
            return Err(e);
        }
    };

    let mut directory = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(get_data_dir().join("directory"))?;

    for embedding in embeddings.iter_mut() {
        embedding.id = get_next_id()?;
        block.embeddings.push(embedding.clone());

        writeln!(
            directory,
            "\n{} {} {}",
            embedding.id, embedding.source_file.filepath, last_block_number
        )?;
    }

    let filepath = format!("{}/{}", get_data_dir().to_str().unwrap(), block.block);
    block.to_file(&filepath)?;

    info!("Saved embedding to {}", filepath);

    info!("Directory updated");

    Ok(())
}
