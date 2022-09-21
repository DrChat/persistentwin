# Persistent Windows
This app is designed to sit in the tray and track both the position of your windows and your display topology.

When the display topology changes (such as by swapping away from the computer with a KVM, or plugging in a dock),
this app will attempt to restore all of your windows to their last observed position on that particular topology.

This works around the issue in Windows 10 where the system will create a "dummy" display if no displays are active,
and reorganize all windows to fit inside of the dummy display (thereby shifting everything away from where you had them).

## Usage
Usage is simple. Just run the app and it will sit in the tray and record all window positioning. When you swap away
and swap back, the app will automatically restore window positions.

## Building
```
cargo build --release
```

## Running
```
cargo run --release
```