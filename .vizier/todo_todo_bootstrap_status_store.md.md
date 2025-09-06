Initialize todos status store on first run so status updates/resolution can proceed without prior state.

Concrete changes tied to current code:

- prompts/src/tools.rs::load_todos()
  • If the backing todos.json file does not exist (io::ErrorKind::NotFound), return Ok(HashMap::new()) instead of Err.
  • Add a companion save_todos(&HashMap<..>) that creates the file and parent dir if missing.

- functions.update_todo_status / functions.read_todo_status callers
  • On first write, call save_todos() after mutation so the file is created.

Acceptance:
- First call to update_todo_status on a fresh repo succeeds and creates todos.json.
- Subsequent reads return the written status.