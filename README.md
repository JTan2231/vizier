# Vizier: LLM Project Management CLI

Vizier is a command-line interface designed for LLM project management, providing functionalities for vector embedding storage, similarity search, and extensible tool integration. The project is built in Rust and organized into a workspace with several crates.

## Overview

Vizier aims to assist developers in managing various aspects of projects that leverage Large Language Models. Its core capabilities include:

* **Vector Embedding Management:** Storing, indexing, and searching high-dimensional vector embeddings, which are crucial for semantic search, retrieval-augmented generation (RAG), and other LLM-related tasks.
* **Extensible Tooling:** A system for defining and registering custom "tools" (Rust functions) that can be introspected and potentially invoked, allowing for flexible project automation and interaction.
* **Workspace Analysis:** Features for analyzing the codebase, such as identifying "TODO" comments.
* **Data Persistence:** Utilizes SQLite for storing metadata and potentially vector data.

## Features

* **Command-Line Interface:** Provides a user-friendly CLI powered by `clap` for interacting with the system[cite: 6].
* **Vector Search:** Implements the HNSW (Hierarchical Navigable Small World) algorithm for efficient approximate nearest neighbor search in vector embedding spaces[cite: 34, 35, 337].
* **Embedding Cache:** Features an LRU (Least Recently Used) cache for quick access to frequently used embeddings[cite: 139, 108].
* **Custom Serialization:** Employs procedural macros for a custom binary serialization format to store embeddings and index structures[cite: 69, 187].
* **Tool Abstraction:** Allows developers to define functions as "tools" with automatic argument struct generation and schema exposure using `vizier-macros`[cite: 12, 578].
* **TODO Detection:** Scans project files to find and list "TODO" comments[cite: 36].
* **SQLite Integration:** Uses SQLite for managing project-related data[cite: 7]. The CLI application initializes a `needs` table[cite: 17].
* **Data Configuration:** Uses a local `.dewey/` directory for storing embedding blocks and index files[cite: 182, 183].

## Project Structure

The Vizier project is a Rust workspace composed of the following main crates[cite: 4]:

* **`vizier` (CLI Application):**
    * The main entry point for the command-line tool[cite: 1].
    * Handles argument parsing, command execution, database initialization, and orchestration of other components.
    * Located in the `cli/` directory.
* **`dewey` (Core Library):**
    * The backbone for vector embedding management.
    * Contains implementations for the embedding cache (`cache.rs`)[cite: 108], HNSW index (`hnsw.rs`)[cite: 337], data I/O operations for embeddings (`dbio.rs`)[cite: 188], and custom serialization logic (`serialization.rs`)[cite: 501].
    * Located in the `dewey/` directory.
* **`dewey-macros` (Procedural Macro Crate):**
    * Provides a custom `#[derive(Serialize)]` macro for the `dewey` library's binary serialization needs[cite: 69, 4].
    * Located in the `dewey-macros/` directory.
* **`vizier-macros` (Procedural Macro Crate):**
    * Provides the `#[tool]` attribute macro used by the `vizier` CLI to define and register tools[cite: 578, 4].
    * Located in the `vizier-macros/` directory.

The project also includes a `.gitignore` file that excludes build artifacts, lock files, and database files from version control[cite: 3].

## Core Components

### Dewey Library

The `dewey` library is central to Vizier's ability to handle vector embeddings.

* **Embedding Storage (`dbio.rs`):**
    * Embeddings are defined with a fixed dimension (`EMBED_DIM` = 1536)[cite: 189].
    * They are stored on disk in `EmbeddingBlock` structures, with a configurable `BLOCK_SIZE` (default: 1024 embeddings per block)[cite: 188, 193].
    * A `Directory` file (`.dewey/directory`) maps embedding IDs and source filepaths to their respective block numbers[cite: 199, 200, 270].
    * Functions are provided to add new embeddings, re-block existing embeddings for optimized access[cite: 216], and read/write blocks and the directory.
* **Embedding Cache (`cache.rs`):**
    * An LRU (Least Recently Used) cache (`EmbeddingCache`) stores `Embedding` objects in memory to reduce disk I/O[cite: 139].
    * When an embedding is requested, if not in the cache, its entire block is loaded[cite: 162].
    * The cache size is configurable and should be a multiple of `BLOCK_SIZE`[cite: 142, 143].
* **HNSW Index (`hnsw.rs`):**
    * Implements the HNSW algorithm for fast, approximate nearest neighbor searches[cite: 337, 338].
    * The index consists of multiple layers, with sparser connections at top layers and denser connections at bottom layers[cite: 336].
    * Supports insertion of new embeddings [cite: 377] and querying for the k-nearest neighbors of a given embedding[cite: 405].
    * The HNSW index can be serialized to and deserialized from a file (e.g., `.dewey/index`)[cite: 455, 458].
* **Custom Serialization (`serialization.rs` & `dewey-macros`):**
    * A custom `Serialize` trait [cite: 501, 502, 503] and a corresponding procedural macro `#[derive(Serialize)]` [cite: 69] are used for converting data structures (like `Embedding`, `EmbeddingBlock`, `HNSW`) into byte vectors for storage and back.
    * This supports primitive types, Strings, Options, Vecs, HashMaps, HashSets, and fixed-size arrays[cite: 504, 508, 514, 536, 544, 554, 109].
    * The derive macro can ignore fields using an `#[ignore]` attribute[cite: 74].

### Vizier CLI (`cli/src/main.rs`)

The `vizier` CLI application ties together the project's functionalities.

* **Argument Parsing:** Uses `clap` to define and parse command-line arguments, such as specifying the path to the SQLite database[cite: 6, 15, 16].
* **Database Initialization:**
    * Opens or creates an SQLite database (default: `./vizier.db`)[cite: 50].
    * Executes an initial SQL script (`SQL_INIT`) to set up necessary tables, including a `needs` table[cite: 17, 18].
* **Tool System:**
    * Leverages the `#[tool]` macro from `vizier-macros` to define functions as discoverable tools[cite: 12, 44].
    * Each tool has an associated schema, generated by the macro, describing its arguments[cite: 13, 587].
    * Tools are collected at runtime using the `inventory` crate[cite: 13, 52]. An example `diff` tool is provided[cite: 44].
* **TODO Management:**
    * The `get_todos` function uses `grep-searcher` and `ignore` to walk the current directory, find files, and search for lines containing "TODO:"[cite: 36, 11, 8].
    * It collects these TODO lines along with preceding and succeeding context lines[cite: 36, 20, 21].
    * Binary files are skipped during the search[cite: 35, 40].

### Macros

* **`dewey-macros::Serialize`**:
    * A procedural derive macro that generates implementations of the custom `to_bytes` and `from_bytes` methods for the `dewey::serialization::Serialize` trait[cite: 69, 95, 96].
    * Supports structs with named or unnamed (tuple) fields, and arrays[cite: 72, 85, 91, 92].
* **`vizier-macros::tool`**:
    * An attribute macro that transforms a Rust function into a "tool"[cite: 578].
    * It automatically generates a struct (e.g., `FunctionNameArgs`) to hold the function's arguments[cite: 580, 581, 585].
    * It creates a `schema()` method for this struct, which produces a JSON representation of the tool's name and default arguments[cite: 587].
    * It registers the tool's metadata (name and schema function) using `inventory::submit!` for runtime collection[cite: 589].

## Getting Started

### Prerequisites

* Rust programming language and Cargo package manager.

### Building

1.  Clone the repository.
2.  Navigate to the project root directory.
3.  Build the project using Cargo:
    ```bash
    cargo build
    ```
    To build in release mode:
    ```bash
    cargo build --release
    ```

## Usage

The primary executable is `vizier`, which will be located in `target/debug/vizier` or `target/release/vizier` after building.

```bash
./target/debug/vizier [OPTIONS]
```

**Options:**

* `--db_path <DB_PATH>`: Specifies the path to the SQLite database file. Defaults to `./vizier.db` if not provided[cite: 15, 50].
* `--version`, `--help`: Display version information or help message.

Currently, the CLI initializes the database and lists registered tools and their schemas[cite: 52].

## Configuration

* **Data Directory:** The `dewey` library stores its data (embedding blocks, index file) in a directory named `.dewey/` relative to the current working directory[cite: 182, 183].
* **Database File:** The SQLite database is named `vizier.db` by default and is created in the current working directory[cite: 50], unless specified otherwise with the `--db_path` option.
* **Ignore File for TODOs:** The TODO search functionality respects a custom ignore file named `vizier.db` (this seems like a specific ignore for the database file itself, but implies general ignore patterns could be supported by `WalkBuilder`)[cite: 38].

## Key Dependencies

* `clap`: For CLI argument parsing[cite: 6].
* `rusqlite`: For SQLite database interaction[cite: 7].
* `sqlite-vec`: SQLite extension for vector operations (listed as a dependency for `cli`)[cite: 7].
* `serde`, `serde_json`: For JSON serialization/deserialization, particularly for tool schemas[cite: 9].
* `inventory`: For collecting tool registrations at runtime[cite: 13].
* `grep-matcher`, `grep-regex`, `grep-searcher`: For file searching (used in TODO detection)[cite: 11].
* `ignore`: For directory walking with respect to ignore files[cite: 8].
* `proc-macro2`, `quote`, `syn`: For procedural macro development (`dewey-macros`, `vizier-macros`)[cite: 55, 575].
* `tree-sitter`, `tree-sitter-rust`, `tree-sitter-python`, `tree-sitter-javascript`: Language parsing libraries (dependencies of `dewey`, suggesting potential future use in code analysis for embeddings)[cite: 105].

## Future Work & Known Issues

The codebase contains several comments indicating areas for improvement or unresolved issues:

* **`cli/src/main.rs`**:
    * The `MatchCollector` for TODOs has a known issue: "TODO: this isn't catching context_before for some reason"[cite: 24].
    * A comment expresses dissatisfaction with checking if a file is binary: "// not happy about having to check whether a given file is a binary"[cite: 40].
    * The `get_tools_context` function is marked with "// TODO: This should probably be generalized at some point"[cite: 46].
* **`dewey/src/cache.rs`**:
    * The LinkedList implementation is acknowledged as mostly "ripped from [https://rust-unofficial.github.io/too-many-lists/sixth-final.html](https://rust-unofficial.github.io/too-many-lists/sixth-final.html)" and "this really could use some cleaning up"[cite: 107, 108].
    * Serialization for the cache is considered: "// TODO: some sort of serialization for the cache // but is it even worth it? how bad are cold starts?"[cite: 137, 138].
    * A call for proper testing: "// TODO: PLEASE god test this properly"[cite: 140].
    * A panic that occurred only in release builds related to fetching embeddings from the cache after loading a block was noted and worked around, but the root cause was unknown: "// TODO: this was triggering panics _only in release builds_ // and I still have no idea why"[cite: 166, 167].
* **`dewey/src/dbio.rs`**:
    * The `get_next_id` function is noted as "NOTE: not thread safe"[cite: 207].
    * Concerns about holding the entire directory in memory: "// TODO: at what point should we worry about holding this whole thing in memory? // it shouldn't stay like this forever"[cite: 265, 266].
* **`dewey/src/hnsw.rs`**:
    * Performance of HNSW implementation: "// TODO: Performance is horrific"[cite: 335].
    * The `insert` method: "// TODO: how? this should be much cleaner"[cite: 378].
    * The query method: "// TODO: please god optimize this", "// TODO: performance optimization? // scaling analysis? // literally anything beyond this leetcode-ass implementation?"[cite: 414, 415, 416].
* **`dewey/src/lib.rs`**:
    * A strong statement about the crate's state: "// TODO: Absolute nightmare crate. This needs to be rewritten as a SQLite plugin or something. // Dear God."[cite: 473, 474].
    * The programmatic `Dewey` struct API is considered old and in need of deprecation/refactoring[cite: 475].
    * A TODO for keeping the HNSW index fresh without compromising performance[cite: 481].
* **`dewey-macros/src/lib.rs`**:
    * The `Serialize` derive macro is described as "// lol // this is hideous"[cite: 69].
