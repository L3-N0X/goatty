Absolutely. In fact, I'd consider this one of the strongest reasons to separate `goatty` and `goattyd` in the first place. I would design goatty as a remote-capable client/server application from day one, even though the first release only connects to a daemon running on the same machine.

What you're describing is essentially:

- goatty: A native desktop application running on your laptop or workstation. It handles the GUI, GPU rendering, keyboard, mouse, clipboard and user interaction.
- goattyd: A headless terminal server running wherever you want. It manages shell processes, PTYs, sessions, installed tools, profiles, plugins and configuration.

You could have goatty running on your Windows laptop, connected to goattyd on your Raspberry Pi, a Linux workstation, a cloud server or a machine at work.

And importantly, the terminal itself would still be rendered locally using your local GPU. You would not stream screenshots or a remote desktop.

This is already a proven architectural pattern. For example, WezTerm supports connecting its desktop GUI to a remote multiplexer daemon over SSH, including spawning the daemon remotely. Eternal Terminal takes a similar client/server approach to persistent, reconnectable terminal sessions.&#x20;

[image](https://www.google.com/s2/favicons?domain=https://wezterm.org&sz=32)

Wez's Terminal Emulator+1

# 1. The architecture I'd aim for

Your laptop

Client

goatty — GPUI

Tabs & panes

GPU rendering

Clipboard

Input handling

Secure connection

SSH tunnel initially · authenticated network protocol later

Remote Linux server

Server

goattyd — Rust

Session manager

Workspaces, tabs, panes

PTY manager

Processes and shells

Terminal core

Rio-VT, screen state

Environment

Zsh, tools, plugins

Profile engine

Configuration

Plugin manager

Installations

Remote execution environment

Zsh · Git · Docker · Python · Your projects · Running servers

The desktop application becomes almost completely independent of the machine executing the commands.

For example, when you run `docker ps`, the Docker CLI is executed on the remote server, not your laptop. Its output is transmitted to goatty and rendered locally.

The same applies to installing plugins: when connected to a server, the plugin manager installs tools on that server.

# 2. One architectural change I'd make now: the terminal state belongs to goattyd

This is the most important technical detail.

There are two ways to implement remote terminal communication.

Option A — Send raw terminal bytes

The server sends PTY output to the desktop, and the desktop uses Rio-VT to interpret it.

This is similar to SSH. It is relatively straightforward and efficient.

However, there is a problem: what happens when the desktop disconnects and later reconnects?

The new desktop instance needs to reconstruct the terminal's current screen and scrollback. You then need output buffering, replay, checkpoints and terminal state restoration.

Option B — Server-authoritative terminal state

This is what I'd recommend for goatty.

Run Rio-VT inside goattyd. The daemon maintains the authoritative terminal state and sends structured updates to the desktop.

For example:

```
goattyd
  |
  ├── PTY output
  |
  ├── Rio-VT
  |     ├── Terminal grid
  |     ├── Cursor
  |     ├── Scrollback
  |     ├── Selection metadata
  |     └── Terminal attributes
  |
  └── Terminal protocol
        |
        └── goatty desktop
              |
              └── GPUI GPU renderer
```

The desktop doesn't need to interpret terminal escape sequences itself. It receives terminal state and draws it.

This is conceptually similar to Mosh, where both endpoints maintain terminal screen state and synchronize changes instead of merely forwarding a byte stream.&#x20;

[image](https://www.google.com/s2/favicons?domain=https://mosh.org&sz=32)

Mosh

Rio's recent separation of `rio-vt` and its embeddable engine makes this direction particularly interesting.&#x20;

[image](https://www.google.com/s2/favicons?domain=https://rioterm.com&sz=32)

Rio Terminal

## How reconnection would work

Imagine you're running a long Docker build on your server.

1. goatty starts a session on goattyd.
2. The server launches Zsh through a PTY.
3. The build starts.
4. You close goatty on your laptop.
5. The server keeps the process alive and continues updating its terminal state.
6. You open goatty again, perhaps on another computer.
7. The new client requests the existing session.
8. goattyd sends the current terminal snapshot and subsequent updates.
9. You continue using the same session.

This is much more powerful than merely restoring a tab layout.

Eternal Terminal demonstrates the same fundamental persistence principle: the remote process survives client disconnection, and reconnecting clients exchange sequence information and missing buffered data.&#x20;

[image](https://www.google.com/s2/favicons?domain=https://github.com&sz=32)

GitHub

# 3. Define a proper communication protocol

I'd introduce a dedicated `goatty-protocol` abstraction between goatty and goattyd.

It shouldn't matter whether the connection uses a local Unix socket, SSH tunnel or a remote network connection.

The desktop should only know that it is connected to a goatty server.

I'd separate communication into two logical channels:

| Control channel        | Terminal data channel    |
| ---------------------- | ------------------------ |
| List workspaces        | Receive terminal updates |
| Create/delete sessions | Send keyboard input      |
| Rename tabs            | Send mouse input         |
| Change profiles        | Resize terminal          |
| Install plugins        | Receive scrollback       |
| Manage packages        | Reconnect to sessions    |
| Get host information   | Send signals             |

The protocol should use versioned, serializable message types, with sequence numbers for terminal updates.

For reconnecting, the client could send its last received sequence number. The server either replays the missing updates or sends a fresh snapshot if the required updates are no longer buffered.

I'd keep the protocol independent of Rio-VT's internal types. Define goatty-owned terminal snapshots and update messages, then let a Rio adapter translate between Rio's internal representation and your protocol. That way, changing the terminal engine doesn't break remote compatibility.

For the actual transport, I'd use:

- Local: Unix domain sockets on Linux/macOS, named pipes on Windows.
- Initial remote implementation: SSH tunnelling.
- Later: authenticated TLS connections, potentially with a WebSocket or other bidirectional transport.
- Future: optimized transport for unreliable connections if needed.

Don't implement your own encryption or authentication protocol. Use established technologies.

# 4. How I'd approach remote connectivity

I would implement remote connectivity in three stages.

Stage 1 · First release

Local daemon connection

goatty launches or connects to goattyd on the same computer.

Everything already uses the client/server protocol, even though both processes run locally.

No network exposure required.

Stage 2 · Remote MVP

Connect through SSH

The desktop connects to an SSH host, establishes a secure tunnel and communicates with goattyd remotely.

The daemon remains bound to localhost on the remote machine. SSH handles authentication and encryption.

No public goattyd port necessary.

Stage 3 · Advanced

Native remote connections

Introduce authenticated TLS connections, host pairing, automatic reconnects and possibly connection roaming.

This could eventually support remote administration without requiring users to manually configure SSH tunnels.

This is not just theoretical: WezTerm already uses an SSH connection to start and connect to a remote multiplexer daemon, with additional TLS-based connection support.&#x20;

[image](https://www.google.com/s2/favicons?domain=https://wezterm.org&sz=32)

Wez's Terminal Emulator+1

I'd also integrate the user's existing `~/.ssh/config`, so connecting to a server could be as simple as selecting `my-server` from a host list.

# 5. A few things to consider for remote sessions

There are some differences between a local terminal and a remote terminal that should influence your architecture.

Terminal resizing

A PTY has one terminal size (columns and rows). If two devices attach to the same session with different screen dimensions, they cannot both independently control its geometry.

For the initial implementation, I'd allow one controlling client per session. Other connections can be read-only observers. The controlling client determines the terminal size.

Clipboard

The clipboard stays on the client.

When you copy text, goatty receives terminal content from goattyd and copies it to your laptop's clipboard. When you paste, the text travels to the server as terminal input.

Local vs remote configuration

This distinction should be explicit:

| Client settings         | Host settings         |
| ----------------------- | --------------------- |
| Window appearance       | Default shell         |
| Font and font size      | Installed tools       |
| Keyboard shortcuts      | Shell plugins         |
| Clipboard behavior      | Prompt configuration  |
| UI layout preferences   | Environment variables |
| Local theme preferences | Profiles and sessions |

This allows you to use the same server from multiple devices without forcing them to have identical appearance settings.

Remote files

A command such as `nano project.txt` works entirely remotely.

But opening a remote file in your laptop's local VS Code editor requires an additional integration, such as VS Code Remote SSH or remote filesystem access. I'd treat this as a future feature, not a core terminal requirement.

Network latency

Initially, use conventional remote input/output streaming through your protocol. It will work perfectly well on ordinary LANs and reasonable internet connections.

Later, you could investigate Mosh-style local input prediction and state synchronization. That can improve responsiveness on high-latency or unreliable connections, but it's a significantly more complicated problem.&#x20;

[image](https://www.google.com/s2/favicons?domain=https://mosh.org&sz=32)

Mosh+1

# 6. What I'd add to the goatty project specification

I wouldn't change the overall technology stack. I'd add a few explicit architectural requirements:

| Component           | New requirement                                   |
| ------------------- | ------------------------------------------------- |
| `goattyd`           | Must run completely headless                      |
| Desktop application | Must be a client, never the owner of sessions     |
| Terminal engine     | Runs in goattyd and owns canonical terminal state |
| Renderer            | Consumes serializable terminal state              |
| Protocol            | Versioned, transport-independent                  |
| Session manager     | Supports detach, attach and reconnect             |
| Configuration       | Distinguishes client and host preferences         |
| Plugin manager      | Installs tools on the selected host               |
| Authentication      | Local permissions initially; SSH for remote MVP   |
| Persistence         | Sessions survive client disconnection             |

I'd also make the remote connection API available to the future CLI and web interface. They should all communicate with the same daemon.

## 7. What the final product could look like

Imagine opening goatty on your laptop.

goatty

Connected

Local machine

Local

Your laptop's own goattyd instance

Home server

Connected

192.168.178.20 · Linux · Zsh

Docker management

Home Assistant development

Workstation

Connected

workstation.example.com · Linux · Zsh

ML experiments

Franka robot project

Add remote host

Illustrative interface concept

You can switch between hosts, workspaces and sessions without changing applications.

The same GUI, keybindings, theme and terminal experience follow you everywhere. The server determines which shell, tools, profiles and projects are available.

# My final recommendation

Architecture decision

Make goatty a remote-first client/server architecture, even if the first release only supports local connections.

I would actually adjust one detail from the original project plan:

Instead of thinking of goattyd as a background helper that launches terminals for the desktop application, treat it as the actual owner of terminal sessions and terminal state.

That gives you a clean separation:

- `goattyd` owns execution, sessions, profiles, tools and plugins.
- `goatty` owns presentation, interaction and GPU rendering.
- `goatty-protocol` connects them independently of their location.

You don't need to implement networking immediately. But I would implement the local communication layer through exactly the same API and serialized message types that you intend to use remotely.

The first release can run everything on one machine. The architecture should never assume that it has to.

That small decision now could save you a substantial refactoring effort later. And it opens up a particularly interesting future for goatty: one polished terminal application that can manage your local machine, home server, development workstation and cloud environments from one interface.
