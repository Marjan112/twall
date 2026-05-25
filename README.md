# twall

twall is a simple TUI program for managing wallpapers

> [!NOTE]
> twall looks inside /usr/share/backgrounds for wallpapers

## Dependencies

- glib development files
- chafa development files
- xwallpaper *on X11*
- on Wayland, only sway compositor is supported (for now)

### Debian

```sh
sudo apt install libglib2.0-dev libchafa-dev
```

#### If on X11

```sh
sudo apt install xwallpaper
```

### Void

```sh
sudo xbps-install glib-devel chafa-devel
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
