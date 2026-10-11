# Utils

Small stand-alone tools for working with AI software. They are **not part of the Jar-vis assistant**: nothing in
`assistant/` depends on them, and they need no build step.

Each tool lives in its own folder with its own README. Prefer the Python standard library only, so a tool runs with
whatever Python 3 the machine already has and needs no virtual environment.

| Tool | What it does |
|---|---|
| [opencode-cleanup](opencode-cleanup/README.md) | Lists opencode sessions with date and size, deletes old ones, and returns the disk space. |
