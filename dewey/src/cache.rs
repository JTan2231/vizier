use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use crate::dbio::{get_directory, read_embedding_block, Embedding, BLOCK_SIZE};
use crate::{error, info};

// TODO: most of this is ripped from https://rust-unofficial.github.io/too-many-lists/sixth-final.html
// this really could use some cleaning up
pub struct LinkedList<T: Clone> {
    front: Link<T>,
    back: Link<T>,
    len: usize,
    _boo: PhantomData<T>,
}

type Link<T> = Option<Arc<Mutex<Node<T>>>>;

pub struct Node<T> {
    elem: T,
    front: Link<T>,
    back: Link<T>,
}

impl<T: Clone> LinkedList<T> {
    pub fn detach(&mut self, node: &Arc<Mutex<Node<T>>>) -> T {
        let node_lock = node.lock().unwrap();
        let front = node_lock.front.clone();
        let back = node_lock.back.clone();
        let elem = node_lock.elem.clone();

        // Update neighboring nodes
        if let Some(front_node) = front.as_ref() {
            let mut front_lock = front_node.lock().unwrap();
            front_lock.back = back.clone();
        } else {
            // This was the back node
            self.back = back.clone();
        }

        if let Some(back_node) = back.as_ref() {
            let mut back_lock = back_node.lock().unwrap();
            back_lock.front = front.clone();
        } else {
            // This was the front node
            self.front = front.clone();
        }

        self.len -= 1;
        elem
    }
}

impl<T: Clone> LinkedList<T> {
    pub fn new() -> Self {
        LinkedList {
            front: None,
            back: None,
            len: 0,
            _boo: PhantomData,
        }
    }

    pub fn push_front(&mut self, elem: T) -> Arc<Mutex<Node<T>>> {
        let new = Arc::new(Mutex::new(Node {
            front: None,
            back: None,
            elem,
        }));

        if let Some(old) = self.front.take() {
            let mut old_lock = old.lock().unwrap();
            old_lock.front = Some(Arc::clone(&new));
        } else {
            self.back = Some(Arc::clone(&new));
        }

        if let Some(front) = &self.front {
            let mut front_lock = front.lock().unwrap();
            front_lock.back = self.back.clone();
        }

        self.front = Some(new.clone());
        self.len += 1;

        new
    }

    pub fn pop_back(&mut self) -> Option<T>
    where
        T: Clone,
    {
        self.back.take().map(|node| {
            let mut node_lock = node.lock().unwrap();
            let result = node_lock.elem.clone();

            self.back = node_lock.front.take();
            if let Some(new_back) = &self.back {
                let mut new_back_lock = new_back.lock().unwrap();
                new_back_lock.back = None;
            } else {
                self.front = None;
            }

            self.len -= 1;
            result
        })
    }
}

impl<T: Clone> Drop for LinkedList<T> {
    fn drop(&mut self) {
        while self.pop_back().is_some() {}
    }
}

// lru: a list of embedding ids
// node_map: a map of embedding ids to their corresponding nodes in the lru
// embeddings: a map of embedding ids to their corresponding embeddings
//
// this relies on $DATA_DIR/directory to find indexed embeddings
//
// TODO: some sort of serialization for the cache
//       but is it even worth it? how bad are cold starts?
pub struct EmbeddingCache {
    lru: LinkedList<u32>,
    node_map: HashMap<u32, Arc<Mutex<Node<u32>>>>,

    // Embeddings that are currently loaded in the cache
    embeddings: HashMap<u32, Embedding>,

    // Embedding ID -> block number
    directory: HashMap<u32, u64>,

    // ideally this is some multiple of the number of embeddings in a block
    // this _must_ be greater or equal to the number of embeddings in a block
    max_size: u32,
}

// TODO: PLEASE god test this properly
impl EmbeddingCache {
    pub fn new(max_size: u32) -> Result<Self, std::io::Error> {
        info!("initializing embedding cache with max size {}", max_size);

        if max_size < BLOCK_SIZE as u32 {
            error!(
                "max_size {} must be greater than or equal to the number of embeddings in a block",
                max_size
            );
            panic!("max_size must be greater than or equal to the number of embeddings in a block");
        }

        let directory = get_directory()?;

        Ok(EmbeddingCache {
            lru: LinkedList::new(),
            node_map: HashMap::new(),
            embeddings: HashMap::new(),
            directory: directory.id_map,
            max_size,
        })
    }

    /// Load an embedding's host block into the cache
    /// This relies on the directory being up to date
    fn load_embedding_block(&mut self, embedding_id: u32) -> Result<(), std::io::Error> {
        let block_number = match self.directory.get(&embedding_id) {
            Some(block_number) => *block_number,
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("embedding {} not found in directory", embedding_id),
                ))
            }
        };

        let embeddings = read_embedding_block(block_number)?.embeddings;
        for e in embeddings.iter() {
            if self.lru.len >= self.max_size as usize {
                let popped = self.lru.pop_back().unwrap();
                self.embeddings.remove(&popped);
                self.node_map.remove(&popped);
            }

            let id = e.id as u32;
            if let Some(node) = self.node_map.get(&id) {
                self.lru.detach(node);
            }

            let new_node = self.lru.push_front(id);
            self.embeddings.insert(id, e.clone());
            self.node_map.insert(id, new_node);
        }

        info!(
            "Loaded {} embeddings, {} embeddings in mapping, {} in LRU",
            embeddings.len(),
            self.embeddings.len(),
            self.lru.len
        );

        Ok(())
    }

    // embedding ids _should_ always be present
    // unless they're not indexed, in which we'd find an io error
    //
    // embeddings are loaded in blocks
    // querying an embedding that's not currently cached will load the entire block into memory
    //
    // cloning the embeddings isn't ideal
    // but neither is the borrow checker
    pub fn get(&mut self, embedding_id: u32) -> Result<Box<Embedding>, std::io::Error> {
        // fetch the embedding
        let embedding = match self.embeddings.get(&embedding_id).cloned() {
            Some(embedding) => embedding,
            None => {
                self.load_embedding_block(embedding_id)?;
                // TODO: this was triggering panics _only in release builds_
                //       and I still have no idea why
                match self.embeddings.get(&embedding_id) {
                    Some(e) => e.clone(),
                    None => {
                        error!(
                            "Dewey: Cache: Embedding id {} doesn't exist in the loaded block!",
                            embedding_id
                        );

                        return Err(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            format!(
                                "Cache panic: Embedding id {} doesn't exist in the blocks!",
                                embedding_id,
                            ),
                        ));
                    }
                }
            }
        };

        let node = self.node_map.get(&embedding_id).unwrap();
        let new_node = self.lru.push_front(embedding_id);

        self.lru.detach(node);

        self.node_map.insert(embedding_id, new_node);

        // TODO: stack + heap allocation? really?
        self.embeddings.entry(embedding_id).and_modify(|e| {
            *e = embedding.clone();
        });

        Ok(Box::new(embedding))
    }

    pub fn refresh_directory(&mut self) -> Result<(), std::io::Error> {
        self.directory = match get_directory() {
            Ok(d) => d.id_map,
            Err(e) => {
                error!("error refreshing cache directory: {}", e);
                return Err(e);
            }
        };

        Ok(())
    }
}
