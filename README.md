# LIFX Control

A small desktop app to control your LIFX lights from Windows over your **local
network** — no LIFX cloud account, no API token, no internet required. It speaks
the LIFX **LAN protocol** directly over UDP using nothing but the Python standard
library (Tkinter for the GUI).

## Requirements

- Python 3 with Tkinter (the standard python.org installer for Windows already
  includes it — verified working on Python 3.14).
- Your PC and your LIFX bulbs on the **same Wi-Fi / subnet**. Discovery uses a
  UDP broadcast, which routers don't forward between subnets or guest networks.

No `pip install` is needed.

## Run

Double-click **`LIFX.bat`**, or from a terminal:

```powershell
python lifx_gui.py
```

The window opens and auto-discovers your lights. Each light gets a card with:

- **On / Off** toggle
- **Brightness** slider (pushed to the bulb when you release it)
- **Color presets** + a full **Color…** picker
- **White** colour-temperature slider (2500–9000 K)

Plus toolbar buttons: **Discover** (re-scan), **All On**, **All Off**.

## Command-line smoke test

To confirm the lights are reachable without the GUI:

```powershell
python lifx_lan.py
```

This broadcasts for devices and prints each one's name, IP, serial, power state,
brightness, colour, and white temperature.

## Files

| File          | Purpose |
|---------------|---------|
| `lifx_lan.py` | The LIFX LAN protocol client (discovery + commands). Importable/scriptable. |
| `lifx_gui.py` | The Tkinter desktop UI. |
| `LIFX.bat`    | Double-click launcher (uses `pythonw`, no console window). |

## Using it as a library

```python
from lifx_lan import LifxLan

lan = LifxLan()
lights = lan.discover()
for light in lights:
    lan.set_power(light, True)
    lan.set_rgb(light, 255, 0, 0)      # red, keeps current brightness
    lan.set_brightness(light, 60)      # 60%
    lan.set_white(light, 80, 2700)     # warm white at 80%
```

## Troubleshooting

- **No lights found:** make sure the bulbs are powered and joined to the same
  network as this PC (not a separate guest/IoT SSID). Some Wi-Fi setups isolate
  IoT devices from PCs — they must be able to exchange UDP on port 56700.
- **A command doesn't take:** UDP can drop packets. The app already re-sends and
  retries; just nudge the control again or hit **Discover** to re-read state.
- **Firewall prompt:** allow Python to communicate on private networks so it can
  receive the bulbs' UDP replies.
```
