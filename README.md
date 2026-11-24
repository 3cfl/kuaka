# Kuaka

<p><a href="https://donate.stripe.com/8x25kFfe96fn0WL6X12wU00"><img src=".github/donate.svg" alt="Donate to Fiordland"></a> <a href="https://discord.gg/gy4ANH2Q9V"><img src=".github/discord.svg" alt="Join the Fiordland Discord"></a></p>

The update app for [Fiordland Linux](https://fiordlandlinux.org). Shows what's waiting, updates the system and Flatpak apps in one click, and puts the system back to an earlier snapshot if an update goes wrong.

- **Updates**: system packages (Fiordland, Arch, CachyOS), Flatpak apps, and AUR packages, grouped and counted. AUR updates open in a terminal so the build scripts can be reviewed first.
- **News**: Fiordland release notes and Arch news that need attention before updating.
- **History**: every snapper snapshot taken before a change, with an Undo button.
- **Tray**: `kuaka --tray` checks in the background and shows a notification when updates are ready.

```
kuaka            open the app
kuaka --tray     background checker for the system tray
kuaka --check    print the number of pending updates
```

## How it works

Pending updates come from `tawaki check --json` (tawaki 0.2.0+). System updates and rollbacks run through `/usr/lib/kuaka/kuaka-helper` via pkexec, which only accepts `upgrade` and `rollback <number>`. Snapshots come from snapper's `root` config and are restored with `fiordland-rollback`.

## Build

```
cargo build --release --locked
```

Install `dist/kuaka-helper` to `/usr/lib/kuaka/`, `dist/lat.apollo44.kuaka.policy` to `/usr/share/polkit-1/actions/`, `dist/kuaka.desktop` to `/usr/share/applications/` and `dist/kuaka-tray.desktop` to `/etc/xdg/autostart/`.

Runtime: `tawaki`, `snapper`, `polkit`, `flatpak` (optional).

## License

GPL-3.0. See [LICENSE](LICENSE).
