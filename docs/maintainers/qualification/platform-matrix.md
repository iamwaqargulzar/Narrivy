# Platform Compatibility Matrix

Linux first-class test dimensions:
- COSMIC Wayland (the 1.0 lane);
- GNOME Wayland (post-release);
- X11/XWayland target apps;
- GTK, Qt, Electron, Chromium, Firefox, terminal, VS Code-class editor, LibreOffice;
- one/multiple monitors;
- mixed DPI;
- suspend/resume;
- microphone hotplug/Bluetooth/USB;
- new PipeWire playback streams during recording;
- Unicode: Latin, RTL, CJK;
- input methods/fallback output backends.

Add KDE/wlroots only with a maintained matrix. Portable crate compile checks run for Windows/macOS continuously.
