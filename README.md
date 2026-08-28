# twall

twall is a simple TUI program for managing wallpapers

> [!NOTE]
> twall looks inside /usr/share/backgrounds and ~/.local/share/backgrounds for wallpapers

# Installation from source

## Dependencies

- rust
- glib development files
- chafa development files
- xwallpaper *on X11*
- on Wayland, only sway compositor is supported (for now)

### Debian

```sh
sudo apt install rustup libglib2.0-dev libchafa-dev
rustup default stable
```

#### If on X11

```sh
sudo apt install xwallpaper
```

### Void

```sh
sudo xbps-install rustup glib-devel chafa-devel
rustup-init
```

#### If on X11

```sh
sudo xbps-install xwallpaper
```

## Installation

```sh
cargo install --path .
```

## License

Licensed under MIT License, see the [LICENSE](./LICENSE) file.
