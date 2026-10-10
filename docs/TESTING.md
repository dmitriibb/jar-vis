# Testing Jar-vis

## Automated

Run from `assistant/app/src-tauri`.

| Command | Covers | Needs |
|---|---|---|
| `cargo test --lib` | Voice command parsing, reply splitting, speech sanitizing, hallucination filter, config, persona sync, pet skins, model catalog and archive extraction, opencode event and message parsing | Nothing |
| `cargo test --test live opencode_round_trip -- --ignored --nocapture` | Service discovery, auth, event stream, agents, session create / list / prompt / delete | opencode running and signed in |
| `cargo test --test live speech_round_trip -- --ignored --nocapture` | Model download and checksum, Piper synthesis, resampling, Whisper transcription | Network on first run (130 MB) |
| `cargo test --test live whisper_handles_silence -- --ignored --nocapture` | Silence is dropped, and Whisper is built optimized | The speech model from the test above |

`opencode_round_trip` fails when the model provider rejects opencode's credentials. Set
`JARVIS_ALLOW_PROVIDER_ERROR=1` to check everything except the model's answer.

Frontend types: `npm run check` in `assistant/app`.

## Manual checklist

Run this on each OS before a release. None of it can be automated, because it depends on real windows, keys and
audio devices.

### Startup

- [ ] First launch opens the wizard, and `config.toml` is created with defaults.
- [ ] The tray icon appears. On macOS there is no Dock icon.
- [ ] A second launch does not start a second instance; it shows the pet.
- [ ] With opencode not installed, the pet shows "offline" and Settings → OpenCode says the binary was not found.
- [ ] With opencode installed but its service stopped (`opencode service stop`), Jar-vis starts it and connects.

### Focus (a showstopper if it fails)

Type continuously in an editor while doing each of these. No keystroke may be lost and the editor must stay focused.

- [ ] An answer arrives and the panel shows itself.
- [ ] A permission request appears.
- [ ] The pet changes state (listening, working, speaking, error).
- [ ] Click the pet, drag the pet, right-click the pet.
- [ ] Click into the panel's text box: now the panel has focus. Press Esc or click away: the panel gives it up.

### Push-to-talk

- [ ] Hold the key, speak, release: the transcript appears and is sent.
- [ ] A tap shorter than 0.3 s is ignored.
- [ ] Pressing another key while holding cancels the recording.
- [ ] Holding the key down for a long time produces one recording (key repeat is ignored).
- [ ] Try each candidate key: Right Ctrl, Right ⌘, F13, Pause. The key still reaches the focused app.
- [ ] Record a new key in Settings; it takes effect without a restart.
- [ ] Windows: the key is not seen while an elevated window has focus (expected, documented).
- [ ] The microphone indicator of the OS is on only while the key is held.
- [ ] Microphone permission denied: the pet shows a clear error, and nothing crashes.

### Turns

- [ ] "What is two plus two" creates a session in `sessions/` and the answer is shown and spoken.
- [ ] The same session is visible in the opencode TUI or web UI and can be continued there.
- [ ] "Start new session" creates a new one; "start new session, list the files" also sends the prompt.
- [ ] Speaking while a turn runs queues the prompt, and it is sent when the turn ends.
- [ ] "Stop" aborts a running turn.
- [ ] A command that needs permission shows the card; Allow once, Always and Deny all work; "yes" and "no" work by voice.
- [ ] Restart opencode in the middle of a turn (`opencode service restart`): Jar-vis goes offline, reconnects and recovers.
- [ ] Restart Jar-vis: it continues in the same session.

### Speech output

- [ ] The summary starts speaking within about a second of the answer arriving.
- [ ] Holding the key while it speaks stops it at once.
- [ ] Mute stops speech and frees the voice model's memory (check in Task Manager or Activity Monitor).
- [ ] "Repeat" and a double-click on the pet replay the last answer.
- [ ] Voice preview in Settings plays each installed voice.

### Settings and models

- [ ] Switching `base.en` to `small.en` downloads it and swaps it without a restart.
- [ ] Cancelling a download leaves no `.part` file behind.
- [ ] Switching personality changes the agent of the current session, the voice and the pet.
- [ ] Editing a file in `assistant/agents` and pressing Re-sync changes the agent's behaviour.

### Attach mode

- [ ] Attach to a session in another project and ask "what did we just change?": the answer uses that session's context.
- [ ] The attached session's own UI shows the voice messages.
- [ ] Detach returns to the previous workspace session.

### Endurance

- [ ] Sleep the machine for a while with Jar-vis running. After waking, the first key press works.
- [ ] After 50 turns, memory use is stable.
- [ ] Left idle for several days, it still responds on the first press.
