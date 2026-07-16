# LumeDial — desktop

A dark, dial-driven desktop app for controlling smart lights over your **local
network** — no cloud account, no API tokens, no internet. Six brands behind one
vendor abstraction, driven by scenes, live effects and screen-sync.

**Works with:** LIFX · Philips Hue · WiZ · Yeelight · Nanoleaf · Govee
*(unofficial — not affiliated with or endorsed by any of these brands)*

## Requirements

- Python 3 with Tkinter (the python.org Windows installer includes it — verified
  on 3.14). No `pip install` needed for the core app.
- Your PC on the **same subnet** as the lights. Discovery binds every local
  interface and sends subnet-directed broadcasts, so it works even with a VPN
  owning the default route.
- Optional: `Pillow` (scene photo fetch + logo generation), `python-vlc` +
  VLC (audio ambience).

## Run

Double-click **`LumeDial.bat`**, or:

```powershell
python lifx_gui.py
```

Hit **Discover**. Lights from every brand appear together, each card tagged with
its vendor.

## Per-brand setup

| Brand | Setup |
|-------|-------|
| **LIFX** | None — discovered automatically. |
| **WiZ** | None — discovered automatically. |
| **Govee** | Enable *LAN Control* for the device in the Govee Home app. |
| **Yeelight** | Enable *LAN Control* for the bulb in the Yeelight app. |
| **Hue** | Press the bridge's round button, then click **+ Hue**. |
| **Nanoleaf** | Hold the panel's power button 5–7s (LEDs flash), then click **+ Nanoleaf**. |

Hue and Nanoleaf credentials are saved to `hue.json` / `nanoleaf.json`, both
gitignored.

## Features

- **33 scenes** with anime artwork; palette scenes re-shuffle colours across
  your bulbs on every tap. Right-click a tile to re-roll its image.
- **Live effects** — breathe, flow, flicker, strobe — that animate the
  *current* scene's colours, capped at each bulb's set brightness, with a live
  speed knob.
- **Screen sync** — samples your screen and drives the lights to complement or
  augment it, with eased colour transitions.
- **Audio ambience** per scene (synth + real recordings).
- **VST-style knobs** everywhere; double-click any knob to reset it.

## Layout

| File | Purpose |
|------|---------|
| `devices.py` | Multi-vendor controller; aggregates + delegates to providers |
| `lifx_lan.py` | LIFX LAN protocol (ack+retry, VPN-aware discovery) |
| `hue.py` `wiz.py` `yeelight.py` `nanoleaf.py` `govee.py` | Per-brand providers |
| `scenes.py` | Scene catalogue, palettes, shuffle |
| `effects.py` | Live effects + screen-sync colour extraction |
| `lifx_gui.py` | The UI |
| `art.py` `fetch_photos.py` | Gradient art + scene photo fetching |
| `ambience.py` | Synthesised audio ambience |
| `branding/gen_logo.py` | Generates the LumeDial logo + all app icons |

## Reliability notes

- One-shot commands (power, colour, scenes, master) are **acknowledged and
  retried**, so every bulb actually receives them. Streaming effects stay
  fire-and-forget for speed.
- Yeelight and Nanoleaf throttle effect streaming to respect their LAN rate
  limits; LIFX, WiZ and Govee stream at full rate.
