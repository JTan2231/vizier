# Vizier 🧙

**Vizier** is an AI-powered project management assistant that lives in your terminal. It's designed to deeply understand your codebase, manage its narrative threads by converting conversations and `TODO`s into concrete tasks, and streamline your development workflow through intelligent git integration.

At its core, Vizier treats software development as a storytelling process. Every `TODO` is a "plot point," and every change moves the project's "narrative" forward.

-----

## ✨ Core Features

  * **🤖 Intelligent Task Management**: Vizier analyzes your project's context—including the file tree and `git diff`—to convert high-level goals and `TODO` comments into actionable, code-aware tasks stored in a local `.vizier/` directory.
  * **🧰 Filesystem & Git Tools**: The assistant is equipped with tools to read/write files, manage TODOs, and inspect the current `git diff`, allowing it to perform meaningful work on your behalf.
  * **💬 Interactive Chat**: A full-featured terminal chat interface (`vizier --chat`) for having conversations with the project assistant, allowing for iterative development and problem-solving.
  * **📝 TUI Task Browser**: An interactive terminal UI (`vizier --list`) for browsing, viewing, and editing the project's TODOs and snapshot.
  * **💾 AI-Powered Commits**: A `--save` command that automatically updates the project snapshot, stages changes, and generates a conventional commit message based on the work done.

-----

## 🚀 How It Works

Vizier operates through a sophisticated, context-aware workflow:

1.  **Context Gathering**: When invoked, Vizier builds a comprehensive snapshot of your project. This includes the file structure, the contents of existing TODOs, and the output of `git diff`.
2.  **Narrative-Driven Prompting**: This context is fed into a powerful system prompt that instructs the LLM to act as a "story editor" for the codebase. Its goal is to resolve narrative tensions (e.g., bugs, missing features) by creating or updating specific, code-anchored tasks.
3.  **Tool-Augmented Execution**: The LLM is given access to a suite of tools that allow it to interact with your project:
      * `list_todos()`, `read_todo(name)`, `add_todo(name, description)`, `update_todo(...)`: Manage task files in the `.vizier/` directory.
      * `read_snapshot()`, `update_snapshot(content)`: Maintain the high-level project narrative.
      * `read_file(path)`: Read source code to inform its decisions.
      * `diff()`: Get the current `git diff`.
      * `update_todo_status(...)`, `read_todo_status(...)`: Manage the lifecycle of tasks.
4.  **Action and Output**: The LLM uses these tools to execute the user's request, resulting in a well-managed `.vizier/` directory that reflects the actionable steps needed to move the project forward.

-----

## 🛠️ Project Structure

Vizier is a Rust workspace composed of three main crates:

  * **`cli`**: The main binary and user entry point. It handles command-line argument parsing, orchestrates the context-gathering process, and manages the interaction with the LLM and TUI.
  * **`prompts`**: Defines the core system prompt, the tools available to the LLM, and file system interaction logic (e.g., building the file tree, finding the project root).
  * **`tui`**: Implements the `ratatui`-based user interfaces for the interactive chat (`--chat`) and the TODO browser (`--list`).

-----

## 🏁 Getting Started

### Prerequisites

  * **Rust & Cargo**: Make sure you have a recent version of the Rust toolchain installed.
  * **Git**: Vizier must be run inside a `git` repository.

### Installation & Build

1.  **Clone the repository:**

    ```bash
    git clone <repository-url>
    cd vizier
    ```

2.  **Build the project:**
    For the best performance, build the project in release mode.

    ```bash
    cargo build --release
    ```

    The final executable will be located at `target/release/vizier`.

-----

## ⚙️ Usage

Vizier can be used for one-off commands or in an interactive session.

| Command                                                    | Description                                                                                                   |
| ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| `vizier [MESSAGE]`                                         | Send a one-off request to the assistant.                                                                      |
| `vizier --chat` or `-c`                                    | Start an interactive chat session in the terminal.                                                            |
| `vizier --list` or `-l`                                    | Open the TUI to browse and manage project TODOs and snapshots.                                                |
| `vizier --summarize` or `-S`                               | Get an LLM-generated summary of all outstanding TODOs.                                                        |
| `vizier --save` or `-s`                                    | **"Save button"**: updates project state, stages changes, and generates a conventional commit with the results. |
| `vizier --provider <anthropic\|openai>` or `-p`            | Specify the LLM provider to use for the request.                                                              |

### Examples

```bash
# Create tasks from all TODO comments in the codebase
./target/release/vizier "Go through the codebase and turn all the TODO comments into tasks."

# Start an interactive chat to refactor a specific module
./target/release/vizier --chat

# Browse existing tasks
./target/release/vizier --list

# Commit all staged changes with an AI-generated message
./target/release/vizier --save
```
