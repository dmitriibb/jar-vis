# opencode-cleanup

Shows your opencode sessions (last used, approximate size, title, folder), lets you pick the old ones, deletes them and
gives the disk space back. One Python file, standard library only (Python 3.8+, tkinter for the window).

```bash
python utils/opencode-cleanup/opencode_cleanup.py                       # window
python utils/opencode-cleanup/opencode_cleanup.py --list                # print sessions, change nothing
python utils/opencode-cleanup/opencode_cleanup.py --before 2026-09-01   # preview, ask, delete
python utils/opencode-cleanup/opencode_cleanup.py --compact             # only shrink the database file
```

`--before 2026-09-01` means "last used before 1 September 00:00, local time". The preview prints the sessions, how many
will go, and the expected size of the database afterwards; nothing is deleted until you type `yes`
(`--yes` skips the question, `--backup` first saves a compact copy of the database next to it, `--no-compact` skips
the last step).

In the window, click **Select** to highlight every session older than the date, adjust the selection with
Ctrl/Shift-click if you like, then **Delete selected...**. Click a column heading to sort.

## What "delete" does

opencode 2 keeps everything in one SQLite file, `opencode.db`, not in per-session folders. The tool:

1. Runs `opencode session delete <id>` for each chosen session. That is opencode's own command, so the running
   background service does the work, and the session's **subagent sessions go with it** (they are listed as a count,
   not separately; a session's "last used" is the newest activity in it or its subagents).
2. Removes what opencode 2 keeps from version 1 and no longer manages: the legacy `session`/`message`/`part`/`todo`
   rows and `storage/session_diff/<id>.json`. This is roughly half of a session's data.
   Sessions you deleted in opencode earlier whose legacy copy is still there are listed as **[leftover]**.
3. Runs `VACUUM`. SQLite never shrinks its file by itself: deleted rows only become unused space inside it, so
   without this step the disk usage does not change.

Sizes in the list are the stored transcript data, not exact disk usage; the "roughly" figure in the preview is
conservative. It needs the `opencode` command on `PATH`.

Deleting sessions while opencode is running is fine. If compacting reports that the database is busy, run
`opencode service stop`, then `--compact`. If the file has not shrunk right after compacting, opencode was in the
middle of reading it; the space comes back at its next checkpoint (`opencode service restart` forces it).

## Where opencode keeps data

Paths are for `~` = your home folder; on macOS and Linux the same layout is used (`XDG_DATA_HOME`, `XDG_CACHE_HOME`
and so on are honoured). Only the first row is touched by this tool; the rest is listed because it is where disk space
goes.

| Path | What | Safe to delete? |
|---|---|---|
| `~/.local/share/opencode/opencode.db` (+ `-wal`, `-shm`) | **All sessions**, messages, tool output, and a cached model catalog (about 6 MB). | Through this tool only. Compacting it can return gigabytes. |
| `~/.local/share/opencode/bin/` | Language servers opencode downloaded for code intelligence (`kotlin-ls`, `jdtls`, `gopls`, ripgrep, node modules). Easily hundreds of MB. | Yes. Re-downloaded when a project needs one. |
| `~/.local/share/opencode/snapshot/<project>/` | One hidden git repo per project, used for undo/revert of file changes. Grows with the project and stays after the sessions are gone. | Yes. You lose revert for those sessions' file changes. |
| `~/.local/share/opencode/storage/session_diff/` | Legacy per-session diff files from opencode 1. | Handled by this tool. |
| `~/.local/share/opencode/log/`, `tool-output/` | Logs; large tool outputs saved aside. | Yes. |
| `~/.cache/opencode/` | Model list cache, plugin packages, and a copy of the service binary named `opencode-service-<pid>-<hash>.exe`. | The binary is in use while the service runs and is recreated; leave it. Others: yes. |
| `~/.config/opencode/node_modules/` | Dependencies of your plugins. | Not worth it; `opencode.json` and `service.json` here are configuration, keep them. |
| `%APPDATA%\opencode\EBWebView\` (Windows) | Web cache of the opencode **desktop app**. | Yes, with the app closed. |
| `~/.local/state/opencode/` | Service address and password, last model. A few bytes. | Keep. |

`~/.local/share/opencode/auth.json` holds your provider credentials; never delete or share it.
