# Vizier 🧙

**Vizier** is an intelligent command-line assistant designed to streamline your software development workflow. It leverages Large Language Models (LLMs) to automatically understand your project, identify pending tasks, and convert `TODO` comments into actionable, organized task files.

At its core, Vizier scans your codebase, constructs a detailed context including your file tree and `git diff`, and uses an LLM to intelligently manage your project's tasks.

## ✨ Core Features

  * **🤖 Automated Task Generation:** Analyzes `TODO` comments in your code and uses an LLM to transform them into well-defined tasks.
  * **🧰 Filesystem Tools:** The LLM is equipped with tools to read files, manage a dedicated `.todos/` directory, and even run `git diff` to stay updated on your project's status.
  * **🧠 Vector-Powered Context:** Utilizes the `dewey` engine to embed and search TODO task files, allowing the assistant to find relevant context for new requests.
  * **💻 Command-Line Interface:** A simple and intuitive CLI for interacting with the assistant.

-----

## 🚀 How It Works

Vizier operates through a sophisticated, multi-step process to provide contextual and actionable project management:

1.  **Context Gathering:** When you run Vizier with a request, it first builds a comprehensive snapshot of your project. This includes:

      * The complete file structure.
      * A list of all outstanding TODO items, stored as individual markdown files in a `.todos/` directory.
      * The output of `git diff` to understand recent changes.
      * The contents of any files relevant to your request.

2.  **Prompting the LLM:** This context is compiled into a detailed system prompt for an LLM (currently configured for **Claude 3.5 Sonnet**). The prompt instructs the model to act as an autonomous technical project manager whose goal is to convert high-level objectives and `TODO`s into concrete tasks.

3.  **Tool-Augmented Generation:** The LLM is given access to a suite of tools that allow it to interact with your local project:

      * `list_todos()`: Lists all task files in the `.todos/` directory.
      * `read_todo(name)`: Reads the content of a specific task file.
      * `add_todo(name, description)`: Creates a new task file with a descriptive markdown body.
      * `update_todo(name, update)`: Appends new information to an existing task.
      * `read_file(filepath)`: Reads a file from your source code.
      * `todo_lookup(query)`: Performs a vector similarity search over the contents of your task files to find the most relevant information.

4.  **Execution and Output:** The LLM uses these tools to create, update, and organize tasks based on your request and the project's context. The final output is a well-managed `.todos/` directory that reflects the actionable steps needed to move the project forward.

-----

## 🛠️ Project Structure

Vizier is a Rust workspace composed of three main crates:

  * **`cli` (Vizier):** The main binary that you interact with. It handles command-line argument parsing, orchestrates the context-gathering process, and manages the interaction with the LLM via the `wire` library.
  * **`dewey`:** The core vector search and storage engine. It's responsible for:
      * **Embedding Storage (`dbio.rs`):** Persists vector embeddings on disk in custom-formatted blocks.
      * **Vector Search (`hnsw.rs`):** Implements the HNSW (Hierarchical Navigable Small World) algorithm for efficient and fast similarity search.
      * **Caching (`cache.rs`):** Features an LRU cache to keep frequently accessed embeddings in memory and reduce disk I/O.
      * **API Interface (`network.rs`):** Communicates with the OpenAI API to generate embeddings for your text.
  * **`dewey-macros`:** A helper crate that provides a procedural derive macro `#[derive(Serialize)]` for `dewey`'s custom binary serialization protocol.

-----

## 🏁 Getting Started

### Prerequisites

  * **Rust & Cargo:** Make sure you have a recent version of the Rust toolchain installed.
  * **OpenAI API Key:** The `dewey` crate requires an OpenAI API key to generate embeddings.

### Installation & Setup

1.  **Clone the repository:**

    ```bash
    git clone <repository-url>
    cd <repository-directory>
    ```

2.  **Set your API key:**
    Export your OpenAI API key as an environment variable. You can add this to your `.bashrc` or `.zshrc` file for convenience.

    ```bash
    export OPENAI_API_KEY="your-api-key-here"
    ```

3.  **Build the project:**
    For the best performance, build the project in release mode.

    ```bash
    cargo build --release
    ```

    The final executable will be located at `target/release/vizier`.

-----

## ⚙️ Usage

To use Vizier, simply call the executable with a user message describing what you want to accomplish.

```bash
./target/release/vizier "Go through the codebase and turn all the TODO comments into tasks."
```

Vizier will then perform its analysis and use the provided tools to populate or update the `.todos/` directory in your project's root.

### Configuration

  * **`.todos/` directory:** All generated task files are stored here as markdown.
  * **`.dewey/` directory:** `dewey` stores its vector index and embedding data in this directory.
  * **`.gitignore`:** It's recommended to add `.todos/` and `.dewey/` to your `.gitignore` file.
