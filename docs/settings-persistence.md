# Zhekarik Strike player settings

Packaged player defaults are installed when the corresponding file is missing.
An existing player's configuration is authoritative: launch layers, legacy
verification, content repair, and content updates preserve its bytes even if
the publisher omits `excluded_from_hash_check`. Removing a default from a later
content manifest does not delete the player's file.

The launcher protects `config.cfg`, `autoexec.cfg`, `userconfig.cfg`,
`joystick.cfg`, `video.txt`, and `videodefaults.txt` directly under `csgo/cfg`,
video files directly under `csgo`, the recovered `cfg/video.txt`,
`cfg/videodefaults.txt` and `cfg/video.cfg`, and those filenames under
`userdata/<numeric account>/730/local/cfg`. Matching is case insensitive and
accepts either Windows or manifest separators. Engine defaults, binaries, VPKs,
maps and integration files keep their existing integrity/update policy.

This policy does not rewrite existing autoexec commands or recover preferences
already lost by an older launcher. Operators must keep required integration
commands separate from player defaults; putting repeated player resets in an
autoexec script will still execute them in the engine.

Pure runtime overlays, including `items_game.txt`, remain unchanged for the
entire owned game session. Elapsed startup time never selects the normal layer.
Normal files are restored through the existing cleanup after the owned game
process exits. This keeps subsequent server connections consistent with the
pure layer installed at launch.

## Verification and delivery

Run `cargo test --manifest-path src-tauri/Cargo.toml --lib settings` for the
settings lifecycle regressions, then the repository's normal complete Windows
acceptance before packaging. A native macOS test build needs a disposable
`src-tauri/icons/icon.png` converted from the tracked ICO; exclude and remove
that generated file before committing. This is test scaffolding, not a release
asset.

Publish only through the existing signed `scripts/release.ps1` flow described
in [RELEASING.md](../RELEASING.md), with an exact versioned Git tag and immutable
release artifacts. Existing user settings do not need a migration or deletion.
Rollback restores the prior launcher binary; never restore packaged settings
over the player's configuration. Older launchers need publisher-side exclusion
flags for these paths before a rollback is safe for preferences.

Release 1.6.20 pins Rust 1.96.0 and Node 24.18.1 to the native Windows
acceptance environment. The 1.6.19 signing run stopped before publication when
the moving `stable` compiler introduced a deprecation denied by Clippy. Its Git
tag remains immutable; 1.6.20 carries the same player-settings and pure-layer
behavior through the reproducible signed release flow.
