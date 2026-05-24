# twall

twall is a simple TUI program for managing wallpapers

> [!NOTE]
> twall looks inside /usr/share/backgrounds for wallpapers

## Dependencies

- glib development files
- chafa development files

### Debian
```sh
apt install libglib2.0-dev libchafa-dev
```

### Void
```sh
xbps-install glib-devel chafa-devel
```

## Installation
```sh
cargo install --path .
```

## License
Licensed under MIT License, see the [LICENSE](./LICENSE) file.
