use ordered_float::OrderedFloat;
use rand::{thread_rng, Rng};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::io::{Read, Write};

use dewey_macros::Serialize;

use crate::cache::EmbeddingCache;
use crate::config::get_data_dir;
use crate::dbio::{get_directory, Embedding, BLOCK_SIZE, EMBED_DIM};
use crate::serialization::Serialize;
use crate::{error, info};

pub fn dot(a: &Embedding, b: &Embedding) -> f32 {
    let mut sum = 0.0;
    let mut i = 0;
    while i < EMBED_DIM {
        sum += a.data[i] * b.data[i]
            + a.data[i + 1] * b.data[i + 1]
            + a.data[i + 2] * b.data[i + 2]
            + a.data[i + 3] * b.data[i + 3];
        i += 4;
    }

    sum
}

pub fn normalize(embedding: &mut Embedding) {
    let mut sum = 0.;
    for i in 0..EMBED_DIM {
        sum += embedding.data[i] * embedding.data[i];
    }

    let sum = sum.sqrt();
    for i in 0..EMBED_DIM {
        embedding.data[i] /= sum;
    }
}

// embedding id -> (neighbor ids, distances)
type Graph = HashMap<u64, Vec<(u64, f32)>>;

pub enum FilterComparator {
    Equal,
    NotEqual,
}

pub struct Filter {
    pub comparator: FilterComparator,
    pub value: String,
}

impl Filter {
    pub fn from_string(input: &String) -> Result<Self, std::io::Error> {
        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.len() != 2 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid filter format",
            ));
        }

        let comparator = match parts[0] {
            "eq" => FilterComparator::Equal,
            "ne" => FilterComparator::NotEqual,
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Invalid comparator",
                ))
            }
        };

        Ok(Filter {
            comparator,
            value: parts[1].to_string(),
        })
    }

    pub fn compare(self: &Self, query: &str) -> bool {
        match self.comparator {
            FilterComparator::Equal => query == self.value,
            FilterComparator::NotEqual => query != self.value,
        }
    }
}

pub struct Query {
    pub embedding: Embedding,
    pub filters: Vec<Filter>,
}

// TODO: Performance is horrific
//
// NOTE: "top" layers (where nodes are most sparse) are the lower indices
//       (e.g., 0, 1, 2, ...)
//       whereas "bottom" layers (where nodes are most abundant are the upper indices
//       (e.g., ..., n - 2, n - 1, n)
#[derive(Serialize)]
#[allow(unused_attributes)]
pub struct HNSW {
    pub size: u32,
    pub layers: Vec<Graph>,
    entry_id: Option<u64>,
    thresholds: Vec<f32>,
}

// NOTE: A few guarantees to note:
//       - The bottom layer must have _all_ registered nodes, regardless of probability
//       - The entry node must be present in _all_ layers

impl HNSW {
    pub fn new(reindex: bool) -> Result<Self, std::io::Error> {
        if !reindex {
            info!("Dewey: HNSW: loading index from disk");
            let hnsw =
                match Self::deserialize(get_data_dir().join("index").to_string_lossy().to_string())
                {
                    Ok(h) => h,
                    Err(e) => match e.kind() {
                        std::io::ErrorKind::NotFound => Self {
                            size: 0,
                            layers: Vec::new(),
                            entry_id: None,
                            thresholds: Vec::new(),
                        },
                        _ => {
                            error!("Error reading index: {}", e);
                            return Err(e);
                        }
                    },
                };

            return Ok(hnsw);
        }

        info!("Dewey: HNSW: building index from block files");

        let directory = get_directory()?;
        let n = directory.len();
        if n == 0 {
            return Ok(Self {
                size: 0,
                layers: Vec::new(),
                entry_id: None,
                thresholds: Vec::new(),
            });
        }

        let m = n.ilog2();
        let l = n.ilog2();
        let p = 1.0 / m as f32;

        info!(
            "building HNSW with \n\tn: {}\n\tm: {}\n\tl: {}\n\tp: {}",
            n, m, l, p
        );

        let thresholds = (1..l)
            .map(|j| p * (1.0 - p).powi((j as i32 - l as i32 + 1).abs()))
            .chain(vec![1.0].into_iter())
            .rev()
            .collect::<Vec<_>>();

        info!("Dewey: Building HNSW with thresholds: {:?}", thresholds);

        // TODO: config param?
        let mut cache = EmbeddingCache::new(20 * BLOCK_SIZE as u32)?;

        let mut entry_id: Option<u64> = None;
        let mut rng = thread_rng();
        let mut layers = vec![HashMap::new(); l as usize];

        // Iterating through all recorded embeddings and inserting them into different layers,
        // depending on rng
        //
        // NOTE: _Every_ embedding must be in some layer.
        //       This is copied + pasted from `fn insert` because I'm not too bright
        for (id, _) in directory.id_map.iter() {
            let prob = rng.gen::<f32>();
            let new_embedding = cache.get(*id as u32)?;

            // If the node is lucky enough to be inserted in one of the upper layers, it is guaranteed
            // a spot in the lower layers through this variable
            let mut carryover = None;
            for (j, layer) in layers.iter_mut().enumerate().rev() {
                let eid = if entry_id.is_none() {
                    Some(new_embedding.id)
                } else {
                    entry_id
                };

                // The index entry id _must_ be present in the index
                if layer.get(&eid.unwrap()).is_none() {
                    let eid_embedding = *cache.get(eid.unwrap() as u32)?;
                    HNSW::insert_into_layer(
                        &mut cache,
                        eid.unwrap(),
                        layer,
                        &eid_embedding,
                        200, // TODO: ????
                    )?;
                }

                if carryover.is_some() {
                    HNSW::insert_into_layer(
                        &mut cache,
                        eid.unwrap(),
                        layer,
                        &new_embedding,
                        200, // TODO: ????
                    )?;
                } else if prob < thresholds[j] || entry_id.is_none() {
                    HNSW::insert_into_layer(
                        &mut cache,
                        eid.unwrap(),
                        layer,
                        &new_embedding,
                        200, // TODO: ????
                    )?;

                    carryover = Some(new_embedding.id);
                }
            }

            if entry_id.is_none() {
                entry_id = Some(new_embedding.id);
            }
        }

        info!("Dewey: HNSW: Layer stats:");
        for (i, layer) in layers.iter().enumerate() {
            info!("Dewey: HNSW: - Layer {} has {} nodes", i, layer.len());
        }

        info!("Dewey: HNSW: finished building index");

        Ok(Self {
            size: n as u32,
            layers,
            entry_id,
            thresholds,
        })
    }

    // NOTE: the directory _needs_ to have been updated
    //       through dbio.rs
    //
    // TODO: how? this should be much cleaner
    pub fn insert(
        &mut self,
        cache: &mut EmbeddingCache,
        embedding: &Embedding,
    ) -> Result<(), std::io::Error> {
        // Recalculations for creating a new layer
        // `l` is otherwise the number of layers present in the index
        if self.layers.is_empty() || self.size.ilog2() > self.layers.len() as u32 {
            let n = std::cmp::max(1, self.size);
            let log = std::cmp::max(1, n.ilog2());
            let p = 1.0 / log as f32;

            let mut new_layer = Graph::new();
            if self.entry_id.is_some() {
                new_layer.insert(self.entry_id.unwrap(), Vec::new());
            }

            self.layers.push(new_layer);
            self.thresholds = (0..log)
                .map(|j| p * (1.0 - p).powi((j as i32 - log as i32 + 1).abs()))
                .rev()
                .collect::<Vec<_>>();
        }

        let mut rng = thread_rng();
        let prob = rng.gen::<f32>();

        // If the node is lucky enough to be inserted in one of the upper layers, it is guaranteed
        // a spot in the lower layers through this variable
        let mut carryover = None;
        for (j, layer) in self.layers.iter_mut().enumerate() {
            let eid = if self.entry_id.is_none() {
                Some(embedding.id)
            } else {
                self.entry_id
            };

            // The index entry id _must_ be present in the index
            if layer.get(&eid.unwrap()).is_none() {
                HNSW::insert_into_layer(
                    cache,
                    eid.unwrap(),
                    layer,
                    &embedding,
                    200, // TODO: ????
                )?;
            }

            // Standard probability check to see if a node is lucky enough to hit the upper layers
            if prob < self.thresholds[j] || self.entry_id.is_none() {
                HNSW::insert_into_layer(
                    cache,
                    eid.unwrap(),
                    layer,
                    &embedding,
                    200, // TODO: ????
                )?;

                carryover = eid;
            }
            // Otherwise, go ahead and insert if the node has already been inserted above
            else if carryover.is_some() {
                HNSW::insert_into_layer(
                    cache,
                    carryover.unwrap(),
                    layer,
                    &embedding,
                    200, // TODO: ????
                )?;
            }
        }

        if self.entry_id.is_none() {
            self.entry_id = Some(embedding.id);
        }

        self.size += 1;

        Ok(())
    }

    fn insert_into_layer(
        cache: &mut EmbeddingCache,
        entry_id: u64,
        layer: &mut Graph,
        query: &Embedding,
        ef: usize,
    ) -> Result<(), std::io::Error> {
        if layer.is_empty() {
            layer.insert(query.id, Vec::new());
            return Ok(());
        }

        let mut visited = HashSet::new();
        let mut candidates: BinaryHeap<Reverse<(OrderedFloat<f32>, u64)>> = BinaryHeap::new();
        let mut results: BinaryHeap<(OrderedFloat<f32>, u64)> = BinaryHeap::new();

        let entry_node = cache.get(entry_id as u32)?;
        let dist = 1.0 - dot(query, &entry_node);
        candidates.push(Reverse((OrderedFloat(dist), entry_id)));
        results.push((OrderedFloat(dist), entry_id));
        visited.insert(entry_id);

        while let Some(Reverse((curr_dist, curr_id))) = candidates.pop() {
            let furthest_dist = results.peek().map(|(d, _)| d.0).unwrap_or(f32::MAX);

            if curr_dist.0 > furthest_dist {
                break;
            }

            if let Some(edges) = layer.get(&curr_id) {
                for &(neighbor_id, _) in edges {
                    if visited.contains(&neighbor_id) {
                        continue;
                    }

                    visited.insert(neighbor_id);
                    let neighbor = cache.get(neighbor_id as u32)?;
                    let dist = 1.0 - dot(query, &*neighbor);

                    if results.len() < ef || dist < furthest_dist {
                        candidates.push(Reverse((OrderedFloat(dist), neighbor_id)));
                        results.push((OrderedFloat(dist), neighbor_id));

                        if results.len() > ef {
                            results.pop();
                        }
                    }
                }
            }
        }

        let mut new_neighbors = Vec::new();
        for (d, id) in results.into_sorted_vec().iter() {
            new_neighbors.push((*id, d.0));

            let other_neighbors = layer.entry(*id).or_insert(Vec::new());
            other_neighbors.push((query.id, d.0));
        }

        let new_node = layer.entry(query.id).or_insert(Vec::new());
        *new_node = new_neighbors.clone();

        Ok(())
    }

    // TODO: please god optimize this
    //       is this better than bfs?
    //
    // TODO: performance optimization?
    //       scaling analysis?
    //       literally anything beyond this leetcode-ass implementation?
    //
    // dfs search through the hnsw
    pub fn query(
        &self,
        cache: &mut EmbeddingCache,
        query: &Query,
        k: usize,
        ef: usize,
    ) -> Vec<(Box<Embedding>, f32)> {
        if self.layers.is_empty() {
            return Vec::new();
        }

        // TODO: ??? a panic? really?
        if ef < k {
            panic!("ef must be greater than k");
        }

        // there's gotta be a better way to blacklist
        let mut visited = vec![false; self.size as usize + 1];
        let mut blacklist = vec![false; self.size as usize + 1];

        // frankly just a stupid way of using this instead of a min heap
        // but rust f32 doesn't have Eq so i don't know how to work with it
        let mut top_k: Vec<(u64, f32)> = Vec::new();

        let mut count = 0;
        for layer in self.layers.iter().rev() {
            let mut current = match top_k.first() {
                Some(k) => k.0,
                None => self.entry_id.unwrap(),
            };

            if layer.is_empty() {
                continue;
            }

            let mut stack = Vec::new();
            stack.push(current);

            while !stack.is_empty() {
                current = stack.pop().unwrap();
                if let Some(current_neighbors) = layer.get(&current) {
                    let mut neighbors = current_neighbors
                        .clone()
                        .into_iter()
                        .filter_map(|(n, _)| {
                            // TODO: the fact that we need to increment/decrement
                            //       the IDs is obscenely stupid
                            if blacklist[n as usize] {
                                return None;
                            }

                            let e_n = cache.get(n as u32).unwrap();

                            if !visited[n as usize] {
                                Some((n, 1.0 - dot(&query.embedding, &e_n)))
                            } else {
                                blacklist[n as usize] = true;
                                None
                            }
                        })
                        .collect::<Vec<_>>();

                    neighbors.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                    for (neighbor, distance) in neighbors {
                        let neighbor = neighbor as usize;
                        if !visited[neighbor] && !blacklist[neighbor] && count < ef {
                            top_k.push((neighbor as u64, distance));

                            stack.push(neighbor as u64);
                            visited[neighbor] = true;
                            count += 1;
                        }

                        if top_k.len() > k {
                            top_k.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                            while top_k.len() > k {
                                top_k.pop();
                            }
                        }

                        if count >= ef {
                            info!(
                                "Dewey: .query(): returning {} results after {} comparisons",
                                top_k.len(),
                                count
                            );
                            return top_k
                                .into_iter()
                                .map(|(node, distance)| (cache.get(node as u32).unwrap(), distance))
                                .collect::<Vec<_>>();
                        }
                    }
                } else {
                    continue;
                }
            }

            top_k.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        }

        info!(
            "Dewey: .query(): returning {} results after {} comparisons",
            top_k.len(),
            count
        );
        top_k.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        top_k
            .into_iter()
            .map(|(node, distance)| (cache.get(node as u32).unwrap(), distance))
            .collect::<Vec<_>>()
    }

    // not the most efficient
    // need to find a workaround the borrow checker
    pub fn remove_node(&mut self, target_id: u64) {
        let layer_targets = self
            .layers
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                let t = l.get(&target_id);
                if t.is_some() {
                    Some((i, t.unwrap().clone()))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        // removing all outgoing edges from neighbors to target_id
        // i.e., removing all edges (neighbor -> target_id)
        for lt in layer_targets {
            for target in lt.1 {
                let neighbors = self.layers[lt.0].get_mut(&target.0).unwrap();
                neighbors.retain(|n| n.0 != target_id);
            }
        }

        // removing all outgoing edges from target_id
        // i.e., removing all edges (target_id -> neighbor)
        for layer in self.layers.iter_mut() {
            layer.retain(|k, _| *k != target_id);
        }

        self.size -= 1;
    }

    pub fn serialize(&self, filepath: &String) -> Result<(), std::io::Error> {
        info!("Dewey: HNSW: serializing index to {}", filepath);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(filepath)?;

        let bytes = self.to_bytes();
        file.write_all(&bytes)?;

        info!("Dewey: HNSW: finished serializing index");

        Ok(())
    }

    pub fn deserialize(filepath: String) -> Result<Self, std::io::Error> {
        info!("Dewey: HNSW: deserializing index from {}", filepath);

        let mut file = std::fs::File::open(filepath.clone())?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;

        let (hnsw, count) = Self::from_bytes(&bytes, 0)?;

        if count <= 4 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid index file",
            ));
        }

        info!("Dewey: HNSW: finished deserializing index");

        Ok(hnsw)
    }

    pub fn get_last_layer(&self) -> Option<&Graph> {
        self.layers.last()
    }

    pub fn print_graph(&self) {
        for (i, layer) in self.layers.iter().enumerate() {
            println!("Layer {} has {} nodes", i, layer.len());
            for (node, neighbors) in layer.iter() {
                println!(
                    "  Node {}: {:?}",
                    node,
                    neighbors.iter().map(|(n, _)| n).collect::<Vec<_>>()
                );
            }
        }
    }
}
