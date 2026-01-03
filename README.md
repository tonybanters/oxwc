# projectwc (name pending)

Minimal Wayland compositor built with Rust and [Smithay](https://github.com/Smithay/smithay).

## Development

```bash
nix develop
cargo run
```

Or spawn a program directly:

```bash
cargo run -- foot
```

## Keybindings

- `Alt+Return` - Spawn terminal (foot)
- `Alt+Q` - Close focused window
- `Alt+D` - Launch rofi
- `Alt+Escape` - Quit compositor
- `Alt+Click` - Drag window

## Roadmap

1. Discuss codebase layout and architecture
2. Implement core features (tiling, workspaces, etc.)
3. Configuration system
