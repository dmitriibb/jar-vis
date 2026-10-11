#!/usr/bin/env python3
"""Delete old opencode sessions and give the disk space back.

Python 3.8+, standard library only. Works on Windows, macOS and Linux.

    python opencode_cleanup.py                       # window: pick sessions, delete
    python opencode_cleanup.py --list                # print sessions, delete nothing
    python opencode_cleanup.py --before 2026-09-01   # preview, ask, delete
    python opencode_cleanup.py --compact             # only shrink the database file

What "delete" does, in order:
  1. `opencode session delete <id>` for each chosen session. This is opencode's own command, so it goes through the
     running background service and also removes the session's subagent (child) sessions.
  2. Removes the leftovers opencode 2 keeps from version 1: rows in the legacy `session`/`message`/`part` tables and
     `storage/session_diff/<id>.json`. opencode 2 no longer manages these, so its own delete does not touch them.
     Sessions that were deleted in opencode earlier but still have such a legacy copy are listed as "[leftover]".
  3. VACUUM. SQLite never shrinks a file by itself: deleted rows only become "free pages" inside it. Without this
     step the disk space is not returned.
"""
from __future__ import annotations

import argparse
import datetime as dt
import os
import queue
import re
import shutil
import sqlite3
import subprocess
import sys
import threading
from dataclasses import dataclass
from pathlib import Path

SESSION_ID = re.compile(r"^ses_[A-Za-z0-9]+$")
MB = 1024 * 1024


class ToolError(Exception):
    """An expected failure with a message that is fine to show the user."""


# ---------------------------------------------------------------- reading

def data_dir() -> Path:
    base = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local" / "share")
    return Path(base) / "opencode"


@dataclass
class Session:
    id: str
    title: str
    directory: str
    updated: int        # ms since epoch: latest activity in this session or any of its subagents
    size: int           # approximate bytes of stored data, this session plus its subagents
    tree: list[str]     # ids that go away together: this session first, then its subagents
    leftover: bool = False   # only the opencode 1 copy exists (deleted in opencode 2, never cleaned up)

    @property
    def subagents(self) -> int:
        return len(self.tree) - 1


@dataclass
class DbStats:
    file_bytes: int     # opencode.db plus its write-ahead log
    free_bytes: int     # space inside the file that is already unused (reclaimed only by VACUUM)


def _file_bytes(db: Path) -> int:
    total = 0
    for suffix in ("", "-wal"):
        try:
            total += os.path.getsize(str(db) + suffix)
        except OSError:
            pass
    return total


def _open_ro(db: Path) -> sqlite3.Connection:
    return sqlite3.connect(f"{db.resolve().as_uri()}?mode=ro", uri=True, timeout=10)


def load_sessions(data: Path) -> tuple[list[Session], DbStats]:
    db = data / "opencode.db"
    if not db.exists():
        raise ToolError(f"No opencode database at {db}")
    con = _open_ro(db)
    try:
        tables = {r[0] for r in con.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        if "session_v2" not in tables:
            raise ToolError("This database has no session_v2 table. This tool is written for opencode 2.")
        cols = "id, parent_id, title, slug, directory, time_updated"
        rows = con.execute(f"SELECT {cols} FROM session_v2").fetchall()
        v2_ids = {r[0] for r in rows}
        if "session" in tables:
            rows += con.execute(f"SELECT {cols} FROM session WHERE id NOT IN (SELECT id FROM session_v2)").fetchall()
        sizes: dict[str, int] = {}
        for table in ("session_message", "message", "part"):   # v2 transcript plus the legacy v1 copy of it
            if table in tables:
                query = f"SELECT session_id, SUM(LENGTH(CAST(data AS BLOB))) FROM {table} GROUP BY session_id"
                for sid, n in con.execute(query):
                    sizes[sid] = sizes.get(sid, 0) + (n or 0)
        page_size = con.execute("PRAGMA page_size").fetchone()[0]
        free_pages = con.execute("PRAGMA freelist_count").fetchone()[0]
    finally:
        con.close()

    diff_dir = data / "storage" / "session_diff"
    by_id = {r[0]: r for r in rows}
    children: dict[str, list[str]] = {}
    roots: list[str] = []
    for r in rows:
        if r[1] and r[1] in by_id:
            children.setdefault(r[1], []).append(r[0])
        else:
            roots.append(r[0])

    sessions = []
    for root in roots:
        tree, stack = [], [root]
        while stack:
            sid = stack.pop()
            if sid in tree:
                continue
            tree.append(sid)
            stack.extend(children.get(sid, []))
        size = 0
        for sid in tree:
            size += sizes.get(sid, 0)
            try:
                size += (diff_dir / f"{sid}.json").stat().st_size
            except OSError:
                pass
        r = by_id[root]
        leftover = root not in v2_ids
        sessions.append(Session(
            id=root,
            title=("[leftover] " if leftover else "") + (r[2] or f"({r[3]})"),
            directory=r[4] or "",
            updated=max(by_id[s][5] for s in tree),
            size=size,
            tree=tree,
            leftover=leftover,
        ))
    return sessions, DbStats(_file_bytes(db), free_pages * page_size)


# ---------------------------------------------------------------- deleting

def _backup(db: Path, say) -> Path:
    target = db.with_name(f"opencode-backup-{dt.datetime.now():%Y%m%d-%H%M%S}.db")
    say(f"Backing up to {target.name} ...")
    con = sqlite3.connect(db, timeout=30, isolation_level=None)
    try:
        con.execute("VACUUM INTO ?", (str(target),))
    finally:
        con.close()
    return target


def _purge_legacy(db: Path, root_ids: list[str]) -> int:
    """Remove opencode-1 rows. Foreign keys cascade to message, part, todo and session_share."""
    con = sqlite3.connect(db, timeout=30, isolation_level=None)
    try:
        if not con.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='session'").fetchone():
            return 0
        con.execute("PRAGMA foreign_keys=ON")
        removed = 0
        con.execute("BEGIN IMMEDIATE")
        try:
            for rid in root_ids:
                cur = con.execute(
                    "WITH RECURSIVE tree(id) AS (SELECT ? UNION SELECT s.id FROM session s JOIN tree t ON s.parent_id = t.id) "
                    "DELETE FROM session WHERE id IN (SELECT id FROM tree)", (rid,))
                removed += cur.rowcount
            con.execute("COMMIT")
        except Exception:
            con.execute("ROLLBACK")
            raise
        return removed
    finally:
        con.close()


def compact(db: Path, say) -> str:
    """VACUUM the database. Returns a note for the user, empty when everything went as planned.

    The database runs in WAL mode: VACUUM first writes the compact copy into the log, and the file only shrinks
    once that log is checkpointed. A checkpoint cannot finish while another process (opencode) is in the middle of
    reading, so we try it, and say so if it did not happen instead of waiting.
    """
    say("Compacting database (this can take a little while) ...")
    con = sqlite3.connect(db, timeout=60, isolation_level=None)
    try:
        con.execute("PRAGMA busy_timeout=5000")
        con.execute("PRAGMA wal_checkpoint(TRUNCATE)")      # start from an empty log, so VACUUM has room to work
        con.execute("PRAGMA busy_timeout=60000")
        con.execute("VACUUM")
        con.execute("PRAGMA busy_timeout=5000")
        busy = con.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchone()[0]
    except sqlite3.OperationalError as e:
        raise ToolError(
            f"Could not compact the database: {e}.\n"
            "opencode is probably busy with it. Run `opencode service stop`, then `python opencode_cleanup.py --compact`."
        ) from e
    finally:
        con.close()
    if busy:
        return ("The database was compacted, but opencode is still reading it, so the file has not shrunk yet "
                "(its log, opencode.db-wal, holds the result). The space comes back the next time opencode "
                "checkpoints; `opencode service restart` does it now.")
    return ""


def delete_sessions(data: Path, sessions: list[Session], *, do_compact=True, backup=False,
                    standalone=False, say=print) -> dict:
    opencode = shutil.which("opencode")
    if not opencode and any(not s.leftover for s in sessions):
        raise ToolError("`opencode` was not found on PATH. It is needed so the running service deletes the sessions.")
    for s in sessions:
        if not SESSION_ID.match(s.id):
            raise ToolError(f"Refusing unexpected session id {s.id!r}")
    db = data / "opencode.db"
    before = _file_bytes(db)
    backup_path = _backup(db, say) if backup else None

    done: list[Session] = []
    failed: list[tuple[Session, str]] = []
    flags = ["--standalone"] if standalone else []
    no_window = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    for i, s in enumerate(sessions, 1):
        say(f"[{i}/{len(sessions)}] Deleting: {s.title}")
        if s.leftover:      # opencode 2 no longer knows this session; only the legacy rows need to go
            done.append(s)
            continue
        try:
            p = subprocess.run([opencode, "session", "delete", *flags, s.id], capture_output=True, text=True,
                               stdin=subprocess.DEVNULL, timeout=180, creationflags=no_window)
            ok, msg = p.returncode == 0, (p.stderr or p.stdout).strip()
        except subprocess.TimeoutExpired:
            ok, msg = False, "timed out after 180 s"
        if ok:
            done.append(s)
        else:
            failed.append((s, msg[-300:]))

    if done:
        say("Removing leftover opencode 1 data ...")
        _purge_legacy(db, [s.id for s in done])
        for s in done:
            for sid in s.tree:
                try:
                    (data / "storage" / "session_diff" / f"{sid}.json").unlink()
                except OSError:
                    pass
    note = compact(db, say) if do_compact else ""
    return {"done": done, "failed": failed, "before": before, "after": _file_bytes(db), "backup": backup_path,
            "note": note}


def compact_only(data: Path, say) -> str:
    db = data / "opencode.db"
    before = _file_bytes(db)
    note = compact(db, say)
    text = f"Database: {fmt_size(before)} -> {fmt_size(_file_bytes(db))}"
    return f"{text}\n{note}" if note else text


# ---------------------------------------------------------------- formatting

def fmt_size(n: float) -> str:
    if n >= 1024 * MB:
        return f"{n / (1024 * MB):.2f} GB"
    if n >= MB:
        return f"{n / MB:.1f} MB"
    return f"{n / 1024:.0f} KB"


def fmt_date(ms: int) -> str:
    return dt.datetime.fromtimestamp(ms / 1000).strftime("%Y-%m-%d %H:%M")


def parse_cutoff(text: str) -> int:
    try:
        day = dt.datetime.strptime(text.strip(), "%Y-%m-%d")
    except ValueError:
        raise ToolError(f"{text!r} is not a date. Use YYYY-MM-DD, for example 2026-09-01.") from None
    return int(day.timestamp() * 1000)   # local midnight: sessions last used before that day begins


def summary(chosen: list[Session], stats: DbStats) -> str:
    subs = sum(s.subagents for s in chosen)
    payload = sum(s.size for s in chosen)
    after = max(stats.file_bytes - stats.free_bytes - payload, 0)
    return (f"{len(chosen)} sessions (+{subs} subagent sessions), about {fmt_size(payload)} of session data.\n"
            f"Database file now: {fmt_size(stats.file_bytes)}, of which {fmt_size(stats.free_bytes)} is already unused.\n"
            f"After deleting and compacting it should be roughly {fmt_size(after)}.")


def result_text(r: dict) -> str:
    lines = [f"Deleted {len(r['done'])} sessions.",
             f"Database: {fmt_size(r['before'])} -> {fmt_size(r['after'])} "
             f"(freed {fmt_size(max(r['before'] - r['after'], 0))})."]
    if r["note"]:
        lines.append(r["note"])
    if r["backup"]:
        lines.append(f"Backup kept at {r['backup']} - delete it when you are happy.")
    for s, msg in r["failed"]:
        lines.append(f"FAILED: {s.title}: {msg or 'no message'}")
    return "\n".join(lines)


# ---------------------------------------------------------------- command line

def print_table(sessions: list[Session]) -> None:
    print(f"{'Last used':16}  {'Size':>9}  {'Subs':>4}  {'Title':44}  Directory")
    for s in sorted(sessions, key=lambda x: x.updated):
        title = s.title if len(s.title) <= 44 else s.title[:41] + "..."
        print(f"{fmt_date(s.updated):16}  {fmt_size(s.size):>9}  {s.subagents:>4}  {title:44}  {s.directory}")


def run_cli(args, data: Path) -> int:
    if args.compact and not args.before:
        print(compact_only(data, print))
        return 0
    sessions, stats = load_sessions(data)
    if args.list or not args.before:
        print_table(sessions)
        print(f"\n{len(sessions)} sessions. Database file {fmt_size(stats.file_bytes)}, "
              f"{fmt_size(stats.free_bytes)} of it unused (a compact would return that).")
        return 0
    cutoff = parse_cutoff(args.before)
    chosen = [s for s in sessions if s.updated < cutoff]
    if not chosen:
        print(f"No sessions were last used before {args.before}.")
        return 0
    print_table(chosen)
    print(f"\nLast used before {args.before} (00:00, local time):\n{summary(chosen, stats)}")
    if not args.yes and input("\nDelete these sessions? This cannot be undone. Type 'yes' to continue: ").strip() != "yes":
        print("Nothing deleted.")
        return 0
    result = delete_sessions(data, chosen, do_compact=not args.no_compact, backup=args.backup,
                             standalone=args.standalone)
    print("\n" + result_text(result))
    return 1 if result["failed"] else 0


# ---------------------------------------------------------------- window

def run_gui(data: Path, standalone: bool) -> int:
    import tkinter as tk
    from tkinter import messagebox, ttk

    root = tk.Tk()
    root.title("opencode session cleanup")
    root.geometry("1050x560")

    state = {"sessions": [], "stats": DbStats(0, 0), "sort": ("updated", True), "busy": False}
    msgs: queue.Queue = queue.Queue()

    info = tk.StringVar()
    cutoff_var = tk.StringVar(value=(dt.date.today() - dt.timedelta(days=30)).isoformat())
    selected_var = tk.StringVar()
    status = tk.StringVar(value="")
    compact_var = tk.BooleanVar(value=True)
    backup_var = tk.BooleanVar(value=False)

    top = ttk.Frame(root, padding=(10, 8))
    top.pack(fill="x")
    ttk.Label(top, textvariable=info, justify="left").pack(anchor="w")

    bar = ttk.Frame(root, padding=(10, 0, 10, 6))
    bar.pack(fill="x")
    ttk.Label(bar, text="Select sessions last used before").pack(side="left")
    ttk.Entry(bar, textvariable=cutoff_var, width=12).pack(side="left", padx=6)
    ttk.Label(bar, text="(YYYY-MM-DD)").pack(side="left")
    select_btn = ttk.Button(bar, text="Select")
    select_btn.pack(side="left", padx=(10, 0))
    all_btn = ttk.Button(bar, text="Select all")
    all_btn.pack(side="left", padx=4)
    clear_btn = ttk.Button(bar, text="Clear")
    clear_btn.pack(side="left")

    columns = {"updated": ("Last used", 130, "w"), "size": ("Size (approx)", 100, "e"),
               "subs": ("Subagents", 80, "e"), "title": ("Title", 380, "w"), "dir": ("Directory", 300, "w")}
    mid = ttk.Frame(root, padding=(10, 0))
    mid.pack(fill="both", expand=True)
    tree = ttk.Treeview(mid, columns=list(columns), show="headings", selectmode="extended")
    scroll = ttk.Scrollbar(mid, orient="vertical", command=tree.yview)
    tree.configure(yscrollcommand=scroll.set)
    tree.pack(side="left", fill="both", expand=True)
    scroll.pack(side="right", fill="y")

    opts = ttk.Frame(root, padding=(10, 6, 10, 0))
    opts.pack(fill="x")
    ttk.Label(opts, textvariable=selected_var).pack(side="left")
    ttk.Checkbutton(opts, text="Back up first (adds one file next to the database)", variable=backup_var).pack(side="right")
    ttk.Checkbutton(opts, text="Compact database afterwards", variable=compact_var).pack(side="right", padx=12)

    btns = ttk.Frame(root, padding=10)
    btns.pack(fill="x")
    delete_btn = ttk.Button(btns, text="Delete selected...")
    delete_btn.pack(side="left")
    compact_btn = ttk.Button(btns, text="Compact database only...")
    compact_btn.pack(side="left", padx=6)
    refresh_btn = ttk.Button(btns, text="Refresh")
    refresh_btn.pack(side="left")
    ttk.Label(btns, textvariable=status).pack(side="left", padx=14)

    sort_keys = {
        "updated": lambda s: s.updated, "size": lambda s: s.size, "subs": lambda s: s.subagents,
        "title": lambda s: s.title.lower(), "dir": lambda s: s.directory.lower(),
    }

    def render(keep=()):
        key, reverse = state["sort"]
        for iid in tree.get_children():
            tree.delete(iid)
        for s in sorted(state["sessions"], key=sort_keys[key], reverse=reverse):
            tree.insert("", "end", iid=s.id, values=(fmt_date(s.updated), fmt_size(s.size), s.subagents,
                                                     s.title, s.directory))
        present = [i for i in keep if tree.exists(i)]
        if present:
            tree.selection_set(present)
        update_selected()

    def sort_by(col):
        key, reverse = state["sort"]
        state["sort"] = (col, not reverse if key == col else col in ("updated", "size", "subs"))
        render(tree.selection())

    for col, (text, width, anchor) in columns.items():
        tree.heading(col, text=text, command=lambda c=col: sort_by(c))
        tree.column(col, width=width, anchor=anchor, stretch=col in ("title", "dir"))

    def chosen():
        by_id = {s.id: s for s in state["sessions"]}
        return [by_id[i] for i in tree.selection() if i in by_id]

    def update_selected():
        c = chosen()
        subs = sum(s.subagents for s in c)
        selected_var.set(f"Selected: {len(c)} of {len(state['sessions'])} sessions (+{subs} subagent sessions), "
                         f"about {fmt_size(sum(s.size for s in c))}")

    def refresh(keep=()):
        try:
            state["sessions"], state["stats"] = load_sessions(data)
        except (ToolError, sqlite3.Error) as e:
            messagebox.showerror("opencode session cleanup", str(e))
            return
        st = state["stats"]
        info.set(f"{data / 'opencode.db'}\nDatabase file {fmt_size(st.file_bytes)}; "
                 f"{fmt_size(st.free_bytes)} of it is unused space that compacting returns to the disk.")
        render(keep)

    def select_before():
        try:
            cutoff = parse_cutoff(cutoff_var.get())
        except ToolError as e:
            messagebox.showerror("opencode session cleanup", str(e))
            return
        ids = [s.id for s in state["sessions"] if s.updated < cutoff]
        tree.selection_set(ids)
        if ids:
            tree.see(ids[0])
        update_selected()

    def set_busy(busy):
        state["busy"] = busy
        for b in (delete_btn, compact_btn, refresh_btn, select_btn, all_btn, clear_btn):
            b.state(["disabled"] if busy else ["!disabled"])

    def work(fn):
        """Run fn(say) on a thread. fn returns the text for the final dialog."""
        set_busy(True)

        def target():
            try:
                msgs.put(("done", fn(lambda m: msgs.put(("say", m)))))
            except (ToolError, sqlite3.Error, OSError) as e:
                msgs.put(("error", str(e)))
        threading.Thread(target=target, daemon=True).start()

    def poll():
        try:
            while True:
                kind, payload = msgs.get_nowait()
                if kind == "say":
                    status.set(payload)
                    continue
                set_busy(False)
                status.set("")
                refresh()
                (messagebox.showinfo if kind == "done" else messagebox.showerror)("opencode session cleanup", payload)
        except queue.Empty:
            pass
        root.after(100, poll)

    def do_delete():
        c = chosen()
        if not c:
            messagebox.showinfo("opencode session cleanup", "Select at least one session first.")
            return
        if not messagebox.askyesno(
                "Delete sessions",
                f"Delete these sessions permanently?\n\n{summary(c, state['stats'])}\n\nThis cannot be undone.",
                icon="warning"):
            return
        do_compact, do_backup = compact_var.get(), backup_var.get()
        work(lambda say: result_text(delete_sessions(
            data, c, do_compact=do_compact, backup=do_backup, standalone=standalone, say=say)))

    def do_compact():
        st = state["stats"]
        if not messagebox.askyesno(
                "Compact database",
                f"Rewrite the database to return {fmt_size(st.free_bytes)} of unused space to the disk?\n\n"
                "No session is deleted. If opencode is busy this can fail; nothing is harmed."):
            return

        work(lambda say: compact_only(data, say))

    select_btn.configure(command=select_before)
    all_btn.configure(command=lambda: (tree.selection_set(tree.get_children()), update_selected()))
    clear_btn.configure(command=lambda: (tree.selection_remove(tree.selection()), update_selected()))
    delete_btn.configure(command=do_delete)
    compact_btn.configure(command=do_compact)
    refresh_btn.configure(command=lambda: refresh(tree.selection()))
    tree.bind("<<TreeviewSelect>>", lambda _e: update_selected())

    refresh()
    poll()
    root.mainloop()
    return 0


def main() -> int:
    p = argparse.ArgumentParser(description="Delete old opencode sessions and reclaim the disk space.")
    p.add_argument("--list", action="store_true", help="print all sessions and exit")
    p.add_argument("--before", metavar="YYYY-MM-DD", help="delete sessions last used before this date (local time)")
    p.add_argument("--compact", action="store_true", help="only compact the database (no sessions deleted)")
    p.add_argument("--no-compact", action="store_true", help="with --before: skip the compact step")
    p.add_argument("--backup", action="store_true", help="with --before: save a compact copy of the database first")
    p.add_argument("--yes", action="store_true", help="with --before: do not ask for confirmation")
    p.add_argument("--standalone", action="store_true",
                   help="have opencode use a private server instead of the background service (for testing)")
    args = p.parse_args()
    data = data_dir()
    try:
        if args.list or args.before or args.compact:
            return run_cli(args, data)
        return run_gui(data, args.standalone)
    except ToolError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    except sqlite3.Error as e:
        print(f"error: database problem: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
