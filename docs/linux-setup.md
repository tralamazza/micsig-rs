# Linux setup

Two things need attention on Linux that do not on macOS: where libusb comes
from, and whether your user is allowed to open the device.

## libusb

`libusb1-sys` bundles libusb and builds it if `pkg-config` cannot find one, so
a C compiler is the only hard requirement — verified on a clean Ubuntu 26.04
image with neither `pkg-config` nor libusb installed, where it statically links
a vendored libusb using the netlink backend. Installing the system libraries is
still preferable, since it links `libusb-1.0.so` and picks up the udev backend:

```
sudo apt install pkg-config libusb-1.0-0-dev libudev-dev   # Debian/Ubuntu
```

## USB permissions

Without a udev rule, libusb cannot open the device and every command fails.
A rule is shipped in this repo at [`contrib/99-micsig.rules`](../contrib/99-micsig.rules):

```
sudo cp contrib/99-micsig.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Then unplug and replug the scope.

`micsig discover` flags a device it can see but cannot open, and the connection
error names the udev rule rather than claiming nothing was found.

Note that `18d1` is Google's vendor ID (the scope runs Android), so an existing
`android-udev-rules` package may already grant access.

## Kernel `usbtmc` driver

The scope's interface 1 is class `FE`/`03`, which Linux's in-tree `usbtmc`
driver binds, creating `/dev/usbtmc0` and holding the interface. `micsig` asks
libusb to auto-detach it on claim and reattach on release, so the two can
coexist; detaching needs the same permissions as above.
