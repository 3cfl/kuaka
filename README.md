# Kuaka

<p><a href="https://donate.stripe.com/8x25kFfe96fn0WL6X12wU00"><img src=".github/donate.svg" alt="Donate to Fiordland"></a> <a href="https://discord.gg/gy4ANH2Q9V"><img src=".github/discord.svg" alt="Join the Fiordland Discord"></a></p>

Created by [Apollo](https://apollo44.lat) ([3cfl](https://github.com/3cfl) on GitHub, u5sk on Discord), as part of [Fiordland Linux](https://github.com/Fiordland-Linux/fiordland).

The update app for [Fiordland Linux](https://fiordlandlinux.org). Shows what's waiting, updates the system and Flatpak apps in one click, and puts the system back to an earlier snapshot if an update goes wrong.

- **Updates**: system packages (Fiordland, Arch, CachyOS), Flatpak apps and AUR packages, grouped and counted, with download sizes for system packages. Packages that need a restart (kernel, firmware, systemd, glibc, Mesa, NVIDIA) are marked. AUR updates open `tawaki update` in a terminal so the build scripts can be reviewed first.
- **Snapshots**: when snapper is set up, kuaka says so before updating, and the history page lists every pre-change snapshot with an Undo button.
- **Progress**: live progress with a details log, and a restart prompt when core packages changed since boot (read from `/var/log/pacman.log`, so it survives closing the app).
- **News**: release notes from `/usr/share/fiordland/news/*.txt`, the notes for a pending Fiordland release, and Arch news posted since the last update.
- **Tray**: `kuaka --tray` checks every 6 hours by default (3, 6, 12 or 24 in settings), waits while on battery or a metered connection, and notifies at most once a day and only about updates it hasn't mentioned before.
- **Looks**: Fiord Expressive (Material 3 Expressive), dark and light following the system. On PCs with less than 4 GB of memory or without GPU acceleration it switches to the software renderer with no translucency, shadows or motion.

```
kuaka            open the app
kuaka --tray     background checker for the system tray
kuaka --check    print the number of pending updates
kuaka --version  print the version
```

Environment overrides: `KUAKA_THEME=dark|light`, `KUAKA_LOW_END=0|1`, `KUAKA_REDUCE_MOTION=0|1`, `KUAKA_SYMBOLS=0|1`.

## How it works

Pending updates come from `tawaki check --json`. Without tawaki, kuaka falls back to `checkupdates` and `flatpak remote-ls --updates` (no AUR).

System updates and rollbacks run through `/usr/lib/kuaka/kuaka-helper` via `/usr/bin/pkexec`. The helper only accepts `upgrade` and `rollback <number>`, uses absolute paths, clears a pacman lock only when it is older than the current boot or no package manager is running, removes HTML error pages that mirrors sometimes serve as database signatures, and uses `-Syuu` on the stable channel, like tawaki. Flatpak apps update as your user. Every other program kuaka starts is called by absolute path with a fixed argument list, never through a shell.

Settings live in `~/.config/kuaka/kuaka.conf` and the tray's notification memory in `~/.local/state/kuaka/`, both in private (0700) folders with 0600 files.

## Build

```
cargo build --release --locked
```

Install `dist/kuaka-helper` to `/usr/lib/kuaka/`, `dist/lat.apollo44.kuaka.policy` to `/usr/share/polkit-1/actions/`, `dist/kuaka.desktop` to `/usr/share/applications/` and `dist/kuaka-tray.desktop` to `/etc/xdg/autostart/`.

Runtime: `tawaki`, `polkit`, `pacman-contrib`, `snapper` and `snap-pac` (optional), `flatpak` (optional), `inter-font` and `ttf-material-symbols-variable` (optional, for the full look).

`cargo build --features shots` adds `kuaka --screenshot DIR PREFIX`, which CI uses to render every screen offscreen.

## License

GPL-3.0. See [LICENSE](LICENSE).
