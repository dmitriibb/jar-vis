# Jar-vis

A small desktop pet that is a voice front-end for [opencode](https://opencode.ai).

Hold one key, speak, release. Jar-vis transcribes your voice on your machine with Whisper, sends the text to an
opencode session, shows what the agent is doing, and reads a short answer back to you with a local Piper voice.
Sessions are ordinary opencode sessions, so you can open the same one in the opencode TUI or web UI and carry on
there.

- **Private:** audio never leaves the machine. Only the transcript goes to opencode.
- **Light:** a Tauri app (Rust core, small Svelte UI). Speech models load lazily and can unload when idle.
- **English only**, for macOS and Windows.

The design is described in [docs/PLAN.md](docs/PLAN.md). Where the implementation differs from it, see
[Differences from the plan](#differences-from-the-plan).

## Status

| Area | State |
|---|---|
| Windows build, unit tests, opencode client, speech round trip | Built and tested |
| A full spoken turn with a real model answer | Needs a signed-in opencode provider; see [Troubleshooting](#troubleshooting) |
| macOS | Written but **not yet built or run on a Mac** |
| Codex backend (plan M9) | Not implemented |

## Requirements

- **opencode 2.x**, installed and signed in to a model provider (`opencode auth login`).
- To build from source:
  - Rust (stable) and Node 20+.
  - The [Tauri 2 prerequisites](https://tauri.app/start/prerequisites/) for your OS.
  - **CMake** and **LLVM/libclang**, needed to build whisper.cpp.
    - Windows: `winget install Kitware.CMake LLVM.LLVM`, then set `LIBCLANG_PATH` to `C:\Program Files\LLVM\bin`.
    - macOS: `xcode-select --install` and `brew install cmake`.

## Build and run

```bash
cd assistant/app
npm install
npm run tauri dev
```

A release build with installers. On Windows use the second command, which also packs the sherpa-onnx DLLs:

```bash
npm run tauri build
```

```bash
npm run bundle:windows
```

To work on everything except speech without CMake and libclang, turn the speech engines off:

```bash
npm run tauri dev -- --no-default-features
```

To work on the UI alone, run only the frontend and open `http://localhost:1420/pet.html`, `/panel.html` or
`/settings.html` in a browser. Outside Tauri, dev builds use a mock backend (`src/lib/mock.ts`); drive it from the
console with `jarvisMock.set({ state: "working", detail: "bash: npm test" })`.

```bash
npm run dev
```

## First run

The setup wizard opens on first launch and walks through:

1. **Home folder.** Where personalities, skills, pets and models live. In a source checkout this is the repo root.
2. **opencode.** Jar-vis finds opencode's background service and starts it if needed.
3. **Permissions.** Microphone on both systems; Accessibility on macOS, for the global key.
4. **Push-to-talk key.** Press the key you want. The default is Right Ctrl on Windows and Right ⌘ on macOS.
5. **Speech models.** Downloads the defaults (about 130 MB).
6. **A test**, then done.

## Using it

| Do this | To |
|---|---|
| Hold the key, speak, release | Send a prompt to the current session |
| Press another key while holding | Cancel the recording |
| Hold the key while Jar-vis talks | Interrupt it and speak |
| Click the pet | Show or hide the panel |
| Double-click the pet | Hear the last answer again |
| Right-click the pet, or use the tray icon | Open the menu |
| Drag the pet | Move it; the position is remembered |

You can also type into the box at the bottom of the panel.

Recording while the agent is busy queues the prompt and sends it when the current turn ends.

### Voice commands

These are handled by Jar-vis itself. Anything else is sent to the agent.

| Say | Effect |
|---|---|
| "start new session", "new chat", … | Start a new session. Words after the phrase become its first prompt. |
| "stop", "cancel", "abort" | Stop the running turn. |
| "mute", "be quiet" / "unmute" | Turn spoken answers off or on. |
| "repeat", "say that again" | Speak the last answer again. |
| "yes", "allow", "do it" / "always" / "no", "deny" | Answer a pending permission request. |

Stop, mute and repeat count only when they are the whole utterance, so "stop the dev server" is still a prompt.
The phrase lists are in the `[commands]` section of the config file.

### The panel

- **Session title ▾** switches between Jar-vis sessions, or starts a new one. The history of the session you switch
  to is loaded from opencode.
- **Model ▾** lists the models opencode offers and switches the current session to the one you pick. In the Jar-vis
  workspace the choice is also remembered for new sessions.
- **⏱** lists the scheduled tasks of every session, with a Cancel button for each.
- **Copy** under an answer copies it. Selecting text and pressing Ctrl+C works too.

### Scheduled tasks

Ask for something to happen later or repeatedly, in your own words:

- "In one hour, check whether my service is running."
- "Check this issue every 15 minutes and tell me the status."
- "At 18:30, remind me to check my soup."

The agent schedules it, and when the time comes Jar-vis sends the stored prompt to the **session it was asked in**,
marked as a scheduled prompt so the model knows the user is not typing it. The answer is shown and spoken like any
other.

- A task belongs to its session. If that session is not the one on screen, the task runs in the background and its
  result is spoken and shown as a notification.
- Repeating tasks stop after 24 hours unless you say otherwise ("for the next three hours", "five times").
- Tasks run only while Jar-vis is running. A task that came due while it was closed or the machine slept fires once
  when it is back; a repeating task then continues from that moment.
- The answer to a scheduled prompt cannot schedule further tasks.
- The rules the agent follows are in `assistant/agents/_schedule-rules.md`. Tasks are stored in `schedule.json`
  next to the logs.

### Permissions

When the agent asks to run a command or edit a file, the pet turns yellow and the panel shows **Allow once**,
**Always** and **Deny**. You can also answer by voice.

## How it works with opencode

opencode 2 runs a per-user **background service**. The opencode TUI and web UI connect to it, and so does Jar-vis,
so a session Jar-vis is driving updates live everywhere else.

- Jar-vis reads the service address and password from `~/.local/state/opencode/service.json`.
- If the service is not running, Jar-vis runs `opencode service start` (turn this off under Settings → OpenCode).
- To use an `opencode serve` instance of your own instead, set its URL and password under Settings → OpenCode.

### Where sessions live

opencode does not store sessions inside the project folder. It keeps them in its own data directory, keyed by project
and directory. `sessions/` is only the **working directory** of Jar-vis sessions. That is what groups them together
and gives them their own agents and skills, so do not be surprised that `sessions/` contains no transcripts.

### Opening a session elsewhere

The panel's ↗ menu and the tray offer:

- **opencode web** — opens the service's web UI, already signed in.
- **opencode TUI** — opens a terminal running `opencode --session <id>` in the session's folder.
- **openchamber** — opens the URL or app set under Settings → OpenCode and copies the session ID.
- **Copy session ID**.

### Attach mode

Settings → Sessions lists every opencode project and its sessions. **Attach** points Jar-vis at one of them, so you
can talk to a session from any other project. The session keeps its own agent; Jar-vis adds a short "voice mode" note
to each message so answers stay speakable. **Detach** returns to the Jar-vis workspace.

## Personalities, skills and pets

Everything you author lives under `assistant/` in the home folder.

- `assistant/agents/<name>.md` is a personality: an opencode agent file with an extra `jarvis:` block for the display
  name, voice, speed and pet.
- `assistant/agents/_voice-rules.md` is appended to every personality. It tells the agent to lead with a short
  spoken summary, then a line containing only `---`, then the details.
- `assistant/skills/<name>/SKILL.md` is an opencode skill.

At startup, and when you press **Re-sync** in Settings, Jar-vis generates `sessions/.opencode/` from these files.
Do not edit that folder. Because the generated config sits inside `sessions/`, it applies to Jar-vis sessions only.

Switch personality from the tray or Settings. The current session switches agent with it.

### Custom pets

The default pet is drawn in code. To make your own, add `assistant/pets/<name>/pet.json` next to its images:

```json
{
  "name": "cat",
  "size": 128,
  "states": {
    "idle":      { "src": "idle.png",   "frames": 8, "fps": 6 },
    "listening": { "src": "listen.png", "frames": 6, "fps": 10, "micLevel": true },
    "working":   { "src": "work.gif" }
  }
}
```

`src` is a horizontal sprite sheet when `frames` is given, otherwise an animated gif, webp or apng. The states are
`idle`, `sleeping`, `listening`, `transcribing`, `sending`, `working`, `permission`, `speaking`, `error` and
`offline`. Missing ones fall back to `idle`.

## Models

Models download into `models/` and are chosen under Settings → Voice input and Voice output.

| Model | Kind | Download | Notes |
|---|---|---|---|
| `tiny.en-q5_1` | speech recognition | 32 MB | Fastest, weakest |
| `base.en-q5_1` | speech recognition | 60 MB | **Default** |
| `small.en-q5_1` | speech recognition | 190 MB | Recommended upgrade |
| `medium.en-q5_0` | speech recognition | 539 MB | |
| `large-v3-turbo-q5_0` | speech recognition | 574 MB | Best quality |
| `piper-en_US-amy-low` | voice | 67 MB | Fast |
| `piper-en_US-lessac-medium` | voice | 67 MB | **Default** |
| `piper-en_GB-alan-medium` | voice | 67 MB | British |
| `piper-en_US-ryan-high` | voice | 116 MB | Better, slower |
| `kokoro-en` | voice | 103 MB | Best quality |

Downloads are checked against a SHA-256 checksum. Muting unloads the voice model, and each model can be set to
unload after a period of idleness.

## Configuration

Settings are saved as you change them. The file is human-editable:

- Windows: `%APPDATA%\jarvis\config.toml`
- macOS: `~/Library/Application Support/jarvis/config.toml`

Logs and runtime state are in `%LOCALAPPDATA%\jarvis` (Windows) or `~/Library/Application Support/jarvis` (macOS).
Settings → Advanced has buttons that open both.

## Troubleshooting

**"opencode could not authenticate with the model provider"** — the provider credential stored in opencode was
rejected. Run `opencode auth login`. Jar-vis itself holds no provider credentials.

**The pet says "offline"** — opencode's service is not reachable. Check `opencode service status`, or use
Settings → OpenCode → Test connection.

**The key does nothing (Windows)** — keys are not seen while a window running as administrator has focus.

**The key does nothing (macOS)** — grant Accessibility access under System Settings → Privacy & Security, then
restart Jar-vis. Unsigned rebuilds lose this permission; signing dev builds with one stable self-signed certificate
avoids that.

**Right Alt types `@` or `€`** — on German and other layouts Right Alt is AltGr. Pick another key.

**The first installer launch is blocked** — builds are unsigned. On Windows choose More info → Run anyway. On macOS
right-click the app and choose Open.

## Tests

```bash
cd assistant/app/src-tauri
cargo test --lib
```

Tests against the real opencode service and real speech models are ignored by default:

```bash
cargo test --test live -- --ignored --test-threads=1 --nocapture
```

The manual checklist is in [docs/TESTING.md](docs/TESTING.md).

## Differences from the plan

`docs/PLAN.md` was written against opencode 1.x. Jar-vis targets opencode 2, whose server changed:

| Plan | Implementation |
|---|---|
| Spawn `opencode serve` on port 4096 | Use opencode's background service, found through `service.json` |
| Endpoints at `/session`, `/event`, … | Endpoints under `/api`, with basic auth |
| `?directory=` scoping | `location[directory]=` |
| `prompt_async` with `agent`, `model`, `system` | `prompt` with `text`; the agent is set on the session |
| `session.idle`, `message.part.updated` events | `session.execution.*`, `session.tool.*`, `session.text.delta` |
| Voice rules sent as `system` in attach mode | Sent as a `[voice mode: …]` prefix on the message |
| `abort` | `interrupt` |
| A model override in Settings | Set `model:` in the personality file |
| Separate `state`, `answer`, `permission` events to the UI | One `snapshot` event carrying the whole UI state |
| `hotkey.suppress` | Present in the config but not implemented; the key always reaches the focused app |
