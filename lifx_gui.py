"""
lifx_gui.py - Slick dark-themed desktop GUI for controlling LIFX lights.

Highlights:
  * Flat dark theme, Bahnschrift / Consolas typography.
  * Scene tiles backed by real photos (images/<scene>.png from fetch_photos.py),
    falling back to procedurally generated gradient art (art.py).
  * VST-style rotary knobs for per-light brightness & white temperature.
  * A master brightness slider that drives every light at once.

Only dependency is the Python stdlib + Tkinter (Pillow is used by the separate
fetch_photos.py tool, not at runtime).

Run:  python lifx_gui.py   (or pythonw lifx_gui.py / double-click LIFX.bat)
"""

import math
import os
import re
import threading
import tkinter as tk
from tkinter import colorchooser, ttk

try:
    import vlc                      # pure-Python binding to installed VLC
except Exception:                   # noqa: BLE001 - missing lib or libvlc
    vlc = None

import art
import lifx_lan
import scenes
from lifx_lan import hsbk_to_rgb
from scenes import SCENES

# --------------------------------------------------------------------------
# Palette & fonts
# --------------------------------------------------------------------------
BG       = "#0d0f14"
PANEL    = "#171a22"
PANEL_HI = "#1e222c"
STROKE   = "#2a2f3b"
TEXT     = "#eceef2"
MUTED    = "#8b93a3"
ACCENT   = "#4cc2ff"
ACCENT_DK= "#2a86c4"
GREEN    = "#3ad29f"
TRACK    = "#2a2f3b"
WARM     = "#ffcf8f"
KNOB_FACE= "#10131a"

F_TITLE   = ("Bahnschrift SemiBold", 18)
F_CARD    = ("Bahnschrift SemiBold", 13)
F_TILE    = ("Bahnschrift SemiBold", 12)
F_CAP     = ("Bahnschrift SemiBold", 8)     # uppercase captions
F_LABEL   = ("Bahnschrift", 10)
F_SMALL   = ("Bahnschrift", 8)
F_READOUT = ("Consolas", 9)                 # digital-style knob value

PRESETS = [
    (255, 45, 45), (255, 130, 0), (255, 220, 0), (0, 210, 80), (0, 200, 220),
    (40, 90, 255), (150, 0, 255), (255, 70, 160), (255, 240, 230),
]

IMG_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "images")
AUDIO_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "audio")


# --------------------------------------------------------------------------
# Helpers
# --------------------------------------------------------------------------

def run_bg(root, fn, on_done=None):
    """Run fn() on a daemon thread; call on_done(result, error) on the UI thread."""
    def worker():
        try:
            result, err = fn(), None
        except Exception as e:        # noqa: BLE001 - surface any network error
            result, err = None, e
        if on_done is not None:
            root.after(0, lambda: on_done(result, err))
    threading.Thread(target=worker, daemon=True).start()


def rgb_hex(r, g, b):
    return f"#{int(r):02x}{int(g):02x}{int(b):02x}"


def round_rect(canvas, x1, y1, x2, y2, r, **kw):
    pts = [x1 + r, y1, x2 - r, y1, x2, y1, x2, y1 + r, x2, y2 - r, x2, y2,
           x2 - r, y2, x1 + r, y2, x1, y2, x1, y2 - r, x1, y1 + r, x1, y1]
    return canvas.create_polygon(pts, smooth=True, **kw)


def flat_button(master, text, command, bg=PANEL_HI, fg=TEXT):
    return tk.Button(master, text=text, command=command, bg=bg, fg=fg,
                     activebackground=ACCENT_DK, activeforeground="#ffffff",
                     disabledforeground=MUTED, relief="flat", bd=0,
                     font=F_LABEL, cursor="hand2", padx=12, pady=5,
                     highlightthickness=0)


def section_label(master, text):
    return tk.Label(master, text=text.upper(), bg=BG, fg=MUTED, font=F_CAP)


def scene_image_path(name):
    safe = re.sub(r"[^a-z0-9]+", "_", name.lower()).strip("_")
    return os.path.join(IMG_DIR, safe + ".png")


# --------------------------------------------------------------------------
# Custom widgets
# --------------------------------------------------------------------------

class Knob(tk.Canvas):
    """A VST-style rotary knob: 270-degree arc, pointer, digital readout.

    Drag up/down or use the scroll wheel to change the value. The on-release
    callback fires the network command; dragging updates the readout live.
    """

    SWEEP = 270
    START = 225          # degrees (Tk: 0=east, CCW+); lower-left

    def __init__(self, master, size=66, value=0, vmin=0, vmax=100,
                 accent=ACCENT, command=None, fmt=None, bg=PANEL):
        super().__init__(master, width=size, height=size, bg=bg,
                         highlightthickness=0, bd=0, cursor="hand2")
        self.size = size
        self.vmin, self.vmax = vmin, vmax
        self.accent, self.command = accent, command
        self.fmt = fmt or (lambda v: f"{int(v)}")
        self._value = value
        self._drag_y = None
        self._drag_v = None
        self._after = None
        self.bind("<Button-1>", self._press)
        self.bind("<B1-Motion>", self._motion)
        self.bind("<ButtonRelease-1>", self._release)
        self.bind("<MouseWheel>", self._wheel)
        self._redraw()

    def _frac(self):
        span = (self.vmax - self.vmin) or 1
        return (self._value - self.vmin) / span

    def _redraw(self):
        self.delete("all")
        s = self.size
        cx = cy = s / 2.0
        m = 7                                  # arc inset from edge
        x0, y0, x1, y1 = m, m, s - m, s - m
        f = self._frac()
        # body
        self.create_oval(cx - (s / 2 - 4), cy - (s / 2 - 4),
                         cx + (s / 2 - 4), cy + (s / 2 - 4),
                         fill=KNOB_FACE, outline=STROKE)
        # arc track + value arc
        self.create_arc(x0, y0, x1, y1, start=self.START, extent=-self.SWEEP,
                        style="arc", outline=TRACK, width=4)
        if f > 0:
            self.create_arc(x0, y0, x1, y1, start=self.START,
                            extent=-self.SWEEP * f, style="arc",
                            outline=self.accent, width=4)
        # pointer
        ang = math.radians(self.START - self.SWEEP * f)
        r1, r2 = s / 2 - 9, s / 2 - 18
        self.create_line(cx + r2 * math.cos(ang), cy - r2 * math.sin(ang),
                         cx + r1 * math.cos(ang), cy - r1 * math.sin(ang),
                         fill="#ffffff", width=3, capstyle="round")
        # digital readout
        self.create_text(cx, cy + 1, text=self.fmt(self._value),
                         fill=TEXT, font=F_READOUT)

    # -- interaction -------------------------------------------------------

    def _press(self, e):
        self._drag_y, self._drag_v = e.y, self._value

    def _motion(self, e):
        if self._drag_y is None:
            return
        span = self.vmax - self.vmin
        delta = (self._drag_y - e.y) / 200.0 * span     # 200px = full range
        self._value = max(self.vmin, min(self.vmax, self._drag_v + delta))
        self._redraw()

    def _release(self, e):
        self._drag_y = None
        if self.command:
            self.command(self._value)

    def _wheel(self, e):
        step = (self.vmax - self.vmin) * 0.04
        step = step if e.delta > 0 else -step
        self._value = max(self.vmin, min(self.vmax, self._value + step))
        self._redraw()
        if self._after:
            self.after_cancel(self._after)
        self._after = self.after(
            220, lambda: self.command and self.command(self._value))
        return "break"          # don't also scroll the page

    def get(self):
        return self._value

    def set(self, v):
        self._value = max(self.vmin, min(self.vmax, v))
        self._redraw()


class SceneTile(tk.Canvas):
    """A scene tile: real photo if available, else gradient art.

    Left-click applies the scene; right-click re-rolls its image (if a reroll
    callback is supplied).
    """

    W, H = 152, 84

    def __init__(self, master, scene, command, reroll=None):
        super().__init__(master, width=self.W, height=self.H, bg=BG,
                         highlightthickness=0, bd=0, cursor="hand2")
        self.scene = scene
        self.command = command
        self.reroll = reroll
        self._img = None
        self._hover = None
        self._busy = None
        self._load_image()
        self._render()
        self.bind("<Enter>", self._enter)
        self.bind("<Leave>", self._leave)
        self.bind("<Button-1>", lambda e: self.command(self.scene))
        if reroll is not None:
            self.bind("<Button-3>", lambda e: self.reroll(self))

    def _load_image(self):
        path = scene_image_path(self.scene["name"])
        if os.path.exists(path):
            try:
                self._img = tk.PhotoImage(file=path)
                return
            except Exception:
                pass
        stops = scenes.tile_stops(self.scene)
        angle = 35 + (sum(ord(c) for c in self.scene["name"]) % 6) * 11
        self._img = tk.PhotoImage(
            data=art.gradient_png_b64(self.W, self.H, stops, angle=angle))

    def _render(self):
        self.delete("all")
        self._hover = self._busy = None
        self.create_image(0, 0, anchor="nw", image=self._img)
        name = self.scene["name"]
        self.create_text(13, self.H - 11, anchor="w", text=name,
                         fill="#0a0a0a", font=F_TILE)        # shadow
        self.create_text(12, self.H - 12, anchor="w", text=name,
                         fill="#ffffff", font=F_TILE)

    def reload_image(self):
        self._load_image()
        self._render()

    def set_busy(self, busy):
        if busy:
            if self._busy is None:
                self.create_rectangle(0, 0, self.W, self.H, fill="#000000",
                                      stipple="gray50", outline="")
                self.create_text(self.W / 2, self.H / 2 - 6, text="re-rolling…",
                                 fill="#ffffff", font=F_CAP)
                self._busy = True
        else:
            self._render()

    def _enter(self, _e):
        if self._hover is None:
            self._hover = self.create_rectangle(1, 1, self.W - 1, self.H - 1,
                                                outline="#ffffff", width=2)

    def _leave(self, _e):
        if self._hover:
            self.delete(self._hover)
            self._hover = None


class LightCard(tk.Frame):
    """A dark card of controls for one bulb."""

    def __init__(self, master, app, light):
        super().__init__(master, bg=PANEL, highlightthickness=1,
                         highlightbackground=STROKE)
        self.app = app
        self.light = light
        pad = tk.Frame(self, bg=PANEL)
        pad.pack(fill="x", padx=14, pady=12)

        # header: swatch | name + ip | toggle
        head = tk.Frame(pad, bg=PANEL)
        head.pack(fill="x")
        self.swatch = tk.Canvas(head, width=34, height=34, bg=PANEL,
                                highlightthickness=0, bd=0)
        self.swatch.pack(side="left")
        meta = tk.Frame(head, bg=PANEL)
        meta.pack(side="left", padx=10)
        self.name_lbl = tk.Label(meta, text=light.label, bg=PANEL, fg=TEXT,
                                 font=F_CARD, anchor="w")
        self.name_lbl.pack(anchor="w")
        self.sub_lbl = tk.Label(meta, text=light.ip, bg=PANEL, fg=MUTED,
                                font=F_SMALL, anchor="w")
        self.sub_lbl.pack(anchor="w")
        self.toggle = Toggle(head, value=light.power, command=self._toggle)
        self.toggle.pack(side="right")

        # rotary knobs: brightness + white temperature
        krow = tk.Frame(pad, bg=PANEL)
        krow.pack(fill="x", pady=(12, 0))
        bbox = tk.Frame(krow, bg=PANEL)
        bbox.pack(side="left", padx=(2, 22))
        self.bri = Knob(bbox, value=light.brightness_pct, accent=ACCENT,
                        command=self._commit_bri, fmt=lambda v: f"{int(v)}%")
        self.bri.pack()
        tk.Label(bbox, text="BRIGHTNESS", bg=PANEL, fg=MUTED, font=F_CAP).pack()
        wbox = tk.Frame(krow, bg=PANEL)
        wbox.pack(side="left")
        self.white = Knob(wbox, value=light.hsbk[3], vmin=2500, vmax=9000,
                          accent=WARM, command=self._commit_white,
                          fmt=lambda v: f"{int(v)}K")
        self.white.pack()
        tk.Label(wbox, text="WHITE", bg=PANEL, fg=MUTED, font=F_CAP).pack()

        # colour presets + picker
        prow = tk.Frame(pad, bg=PANEL)
        prow.pack(fill="x", pady=(12, 0))
        for rgb in PRESETS:
            sw = tk.Canvas(prow, width=22, height=22, bg=PANEL,
                           highlightthickness=0, bd=0, cursor="hand2")
            round_rect(sw, 2, 2, 20, 20, 6, fill=rgb_hex(*rgb), outline="")
            sw.bind("<Button-1>", lambda e, c=rgb: self.apply_rgb(*c))
            sw.pack(side="left", padx=(0, 6))
        flat_button(prow, "Color…", self.pick_color).pack(side="left",
                                                               padx=(4, 0))

        self.refresh_widgets()

    # -- UI sync -----------------------------------------------------------

    def refresh_widgets(self):
        self.name_lbl.config(text=self.light.label or self.light.serial)
        self.toggle.set(self.light.power)
        self.bri.set(self.light.brightness_pct)
        self.white.set(max(2500, min(9000, self.light.hsbk[3])))
        self._paint_swatch()

    def _paint_swatch(self):
        r, g, b = hsbk_to_rgb(*self.light.hsbk)
        if not self.light.power:
            r, g, b = r // 4, g // 4, b // 4
        self.swatch.delete("all")
        round_rect(self.swatch, 2, 2, 32, 32, 9, fill=rgb_hex(r, g, b),
                   outline=STROKE)

    # -- actions (network on a background thread) --------------------------

    def _toggle(self, on):
        run_bg(self.app.root, lambda: self.app.lan.set_power(self.light, on),
               lambda _r, e: self._done(e))

    def _commit_bri(self, v):
        pct = int(v)
        run_bg(self.app.root,
               lambda: self.app.lan.set_brightness(self.light, pct),
               lambda _r, e: self._done(e))

    def _commit_white(self, v):
        k, pct = int(v), int(self.bri.get())
        run_bg(self.app.root,
               lambda: self.app.lan.set_white(self.light, pct, k),
               lambda _r, e: self._done(e))

    def apply_rgb(self, r, g, b):
        run_bg(self.app.root,
               lambda: self.app.lan.set_rgb(self.light, r, g, b),
               lambda _r, e: self._done(e))

    def pick_color(self):
        rgb, _hex = colorchooser.askcolor(
            parent=self, title=f"Color for {self.light.label}")
        if rgb:
            self.apply_rgb(int(rgb[0]), int(rgb[1]), int(rgb[2]))

    def _done(self, err):
        if err:
            self.app.set_status(f"Error: {err}", error=True)
        self.refresh_widgets()


class Toggle(tk.Canvas):
    """An iOS-style sliding on/off switch."""

    def __init__(self, master, value=False, command=None, bg=PANEL):
        super().__init__(master, width=48, height=26, bg=bg,
                         highlightthickness=0, bd=0, cursor="hand2")
        self._on = bool(value)
        self.command = command
        self.bind("<Button-1>", self._click)
        self._redraw()

    def _redraw(self):
        self.delete("all")
        round_rect(self, 3, 4, 45, 22, 9,
                   fill=GREEN if self._on else "#39404f", outline="")
        kx = 35 if self._on else 13
        self.create_oval(kx - 8, 5, kx + 8, 21, fill="#ffffff", outline="")

    def _click(self, _e):
        self._on = not self._on
        self._redraw()
        if self.command:
            self.command(self._on)

    def set(self, v):
        self._on = bool(v)
        self._redraw()

    def get(self):
        return self._on


class Ambience:
    """Loops a per-scene ambient clip via VLC, with live volume control.

    Prefers a real recording (audio/<key>.ogg), then a synth loop
    (audio/<key>.wav), then a synth simulacrum named by scenes.FALLBACK.
    """

    def __init__(self):
        self.enabled = True
        self.volume = 60
        self.current = None
        self._inst = None
        self._player = None
        if vlc is not None:
            try:
                self._inst = vlc.Instance("--quiet", "--input-repeat=999999")
                self._player = self._inst.media_player_new()
                self._player.audio_set_volume(self.volume)
            except Exception:
                self._inst = self._player = None

    def _resolve(self, key):
        for ext in (".ogg", ".wav"):
            p = os.path.join(AUDIO_DIR, key + ext)
            if os.path.exists(p):
                return p
        fb = scenes.FALLBACK.get(key)
        if fb:
            p = os.path.join(AUDIO_DIR, fb + ".wav")
            if os.path.exists(p):
                return p
        return None

    def play(self, key):
        self.current = key
        if not self.enabled or self._player is None:
            return
        path = self._resolve(key)
        if not path:
            return
        try:
            self._player.set_media(self._inst.media_new(path))
            self._player.audio_set_volume(self.volume)
            self._player.play()
        except Exception:
            pass

    def stop(self):
        if self._player is not None:
            try:
                self._player.stop()
            except Exception:
                pass

    def set_volume(self, v):
        self.volume = int(max(0, min(100, v)))
        if self._player is not None:
            try:
                self._player.audio_set_volume(self.volume)
            except Exception:
                pass

    def set_enabled(self, on):
        self.enabled = on
        if on and self.current:
            self.play(self.current)
        elif not on:
            self.stop()


# --------------------------------------------------------------------------
# Application
# --------------------------------------------------------------------------

class App:
    def __init__(self, root):
        self.root = root
        self.lan = lifx_lan.LifxLan()
        self.audio = Ambience()
        self.cards = []
        self.tiles = []

        root.title("LIFX Control")
        root.configure(bg=BG)
        root.geometry("544x820")
        root.minsize(496, 560)

        self._init_style()
        self._build_toolbar()
        self._build_master()
        self._build_scroll()
        self._build_scenes()

        root.protocol("WM_DELETE_WINDOW", self._on_close)
        root.after(250, self.discover)

    def _init_style(self):
        style = ttk.Style()
        try:
            style.theme_use("clam")
        except tk.TclError:
            pass
        style.configure("LIFX.Vertical.TScrollbar", troughcolor=BG,
                        background=PANEL_HI, bordercolor=BG, arrowcolor=MUTED,
                        relief="flat", borderwidth=0)
        style.map("LIFX.Vertical.TScrollbar",
                  background=[("active", ACCENT_DK)])

    def _build_toolbar(self):
        bar = tk.Frame(self.root, bg=BG)
        bar.pack(fill="x", padx=16, pady=(14, 4))
        tk.Label(bar, text="LIFX ", bg=BG, fg=TEXT, font=F_TITLE).pack(side="left")
        tk.Label(bar, text="Control", bg=BG, fg=ACCENT, font=F_TITLE).pack(side="left")
        self.refresh_btn = flat_button(bar, "Discover", self.discover)
        self.refresh_btn.pack(side="right")
        flat_button(bar, "All Off", lambda: self.all_power(False)).pack(
            side="right", padx=6)
        flat_button(bar, "All On", lambda: self.all_power(True),
                    bg=ACCENT_DK, fg="#ffffff").pack(side="right")
        self.sound_btn = flat_button(bar, "Sound: On", self._toggle_sound)
        self.sound_btn.pack(side="right", padx=6)
        self.status = tk.Label(self.root, text="Ready.", bg=BG, fg=MUTED,
                               font=F_LABEL, anchor="w")
        self.status.pack(fill="x", padx=18)

    def _build_master(self):
        box = tk.Frame(self.root, bg=PANEL_HI)
        box.pack(fill="x", padx=12, pady=(6, 0))
        inner = tk.Frame(box, bg=PANEL_HI)
        inner.pack(fill="x", padx=16, pady=12)
        self.master = Knob(inner, size=74, value=100, accent=ACCENT,
                           bg=PANEL_HI, command=self._commit_master,
                           fmt=lambda v: f"{int(v)}%")
        self.master.pack(side="left")
        txt = tk.Frame(inner, bg=PANEL_HI)
        txt.pack(side="left", padx=16)
        tk.Label(txt, text="MASTER", bg=PANEL_HI, fg=TEXT,
                 font=F_CARD, anchor="w").pack(anchor="w")
        tk.Label(txt, text="BRIGHTNESS · ALL LIGHTS", bg=PANEL_HI, fg=MUTED,
                 font=F_CAP, anchor="w").pack(anchor="w", pady=(2, 0))

        vbox = tk.Frame(inner, bg=PANEL_HI)
        vbox.pack(side="right")
        self.vol = Knob(vbox, size=58, value=self.audio.volume, accent=GREEN,
                        bg=PANEL_HI, command=self._set_volume,
                        fmt=lambda v: f"{int(v)}")
        self.vol.pack()
        tk.Label(vbox, text="VOLUME", bg=PANEL_HI, fg=MUTED,
                 font=F_CAP).pack(pady=(2, 0))

    def _build_scroll(self):
        wrap = tk.Frame(self.root, bg=BG)
        wrap.pack(fill="both", expand=True, padx=10, pady=10)
        self.canvas = tk.Canvas(wrap, bg=BG, highlightthickness=0, bd=0)
        sb = ttk.Scrollbar(wrap, orient="vertical",
                           style="LIFX.Vertical.TScrollbar",
                           command=self.canvas.yview)
        self.content = tk.Frame(self.canvas, bg=BG)
        self.content.bind(
            "<Configure>",
            lambda e: self.canvas.configure(scrollregion=self.canvas.bbox("all")))
        self._win = self.canvas.create_window((0, 0), window=self.content,
                                              anchor="nw")
        self.canvas.bind(
            "<Configure>",
            lambda e: self.canvas.itemconfigure(self._win, width=e.width))
        self.canvas.configure(yscrollcommand=sb.set)
        self.canvas.pack(side="left", fill="both", expand=True)
        sb.pack(side="right", fill="y")
        self.canvas.bind_all("<MouseWheel>", self._on_wheel)

        self.scenes_sec = tk.Frame(self.content, bg=BG)
        self.scenes_sec.pack(fill="x")
        self.lights_sec = tk.Frame(self.content, bg=BG)
        self.lights_sec.pack(fill="x", pady=(4, 0))

    def _on_wheel(self, event):
        self.canvas.yview_scroll(int(-event.delta / 120), "units")

    def _build_scenes(self):
        hdr = tk.Frame(self.scenes_sec, bg=BG)
        hdr.pack(fill="x", padx=8, pady=(4, 8))
        section_label(hdr, "Scenes").pack(side="left")
        tk.Label(hdr, text="left-click apply · right-click re-roll image",
                 bg=BG, fg=MUTED, font=F_SMALL).pack(side="right")
        grid = tk.Frame(self.scenes_sec, bg=BG)
        grid.pack()
        cols = 3
        for i, sc in enumerate(SCENES):
            tile = SceneTile(grid, sc, self.apply_scene, reroll=self.reroll_scene)
            tile.grid(row=i // cols, column=i % cols, padx=6, pady=6)
            self.tiles.append(tile)

    # -- status ------------------------------------------------------------

    def set_status(self, text, error=False):
        self.status.config(text=text, fg="#ff6b6b" if error else MUTED)

    # -- discovery ---------------------------------------------------------

    def discover(self):
        self.refresh_btn.config(state="disabled")
        self.set_status("Discovering lights on the LAN …")
        run_bg(self.root, self.lan.discover, self._on_discovered)

    def _on_discovered(self, lights, err):
        self.refresh_btn.config(state="normal")
        for c in self.cards:
            c.destroy()
        self.cards = []
        for w in self.lights_sec.winfo_children():
            w.destroy()
        section_label(self.lights_sec, "Lights").pack(anchor="w", padx=8,
                                                      pady=(10, 8))
        if err:
            self.set_status(f"Discovery failed: {err}", error=True)
            return
        if not lights:
            tk.Label(self.lights_sec,
                     text="No lights found. Make sure they're powered and on "
                          "the same network/subnet as this PC.",
                     bg=BG, fg=MUTED, font=F_LABEL, wraplength=460,
                     justify="left").pack(anchor="w", padx=8)
            self.set_status("No lights found.", error=True)
            return
        for light in lights:
            card = LightCard(self.lights_sec, self, light)
            card.pack(fill="x", padx=6, pady=6)
            self.cards.append(card)
        avg = round(sum(l.brightness_pct for l in lights) / len(lights))
        self.master.set(avg)
        self.set_status(f"Found {len(lights)} "
                        f"light{'s' if len(lights) != 1 else ''}.")

    # -- scene / master actions -------------------------------------------

    def apply_scene(self, scene):
        if not self.cards:
            self.set_status("No lights yet — hit Discover.", error=True)
            return
        lights = [c.light for c in self.cards]
        self.set_status(f"Applying scene: {scene['name']} …")

        def work():
            scenes.apply_scene(self.lan, lights, scene)

        def done(_r, e):
            self.set_status(f"Error: {e}" if e else f"Scene: {scene['name']}",
                            error=bool(e))
            for c in self.cards:
                c.refresh_widgets()
            if not e:
                self.audio.play(scenes.scene_ambience(scene))
        run_bg(self.root, work, done)

    def _toggle_sound(self):
        on = not self.audio.enabled
        self.audio.set_enabled(on)
        self.sound_btn.config(text=f"Sound: {'On' if on else 'Off'}")

    def _set_volume(self, v):
        self.audio.set_volume(v)

    def reroll_scene(self, tile):
        name = tile.scene["name"]
        tile.set_busy(True)
        self.set_status(f"Re-rolling image: {name} …")

        def work():
            import fetch_photos        # lazy: pulls in Pillow only on demand
            return fetch_photos.fetch_scene(tile.scene, randomize=True)

        def done(path, e):
            if e:
                tile.set_busy(False)
                self.set_status(f"Re-roll failed: {e}", error=True)
            elif not path:
                tile.set_busy(False)
                self.set_status(f"No new image found for {name}.", error=True)
            else:
                tile.reload_image()
                self.set_status(f"New image for {name}.")
        run_bg(self.root, work, done)

    def all_power(self, on):
        if not self.cards:
            self.set_status("No lights yet — hit Discover.", error=True)
            return
        self.set_status(f"Turning all lights {'on' if on else 'off'} …")

        def work():
            for c in self.cards:
                self.lan.set_power(c.light, on)

        def done(_r, e):
            self.set_status(f"Error: {e}" if e else
                            f"All lights {'on' if on else 'off'}.",
                            error=bool(e))
            for c in self.cards:
                c.refresh_widgets()
        run_bg(self.root, work, done)

    def _commit_master(self, v):
        if not self.cards:
            return
        pct = int(v)

        def work():
            for c in self.cards:
                self.lan.set_brightness(c.light, pct)

        def done(_r, e):
            self.set_status(f"Error: {e}" if e else f"Master brightness {pct}%.",
                            error=bool(e))
            for c in self.cards:
                c.refresh_widgets()
        run_bg(self.root, work, done)

    def _on_close(self):
        self.audio.stop()
        self.lan.close()
        self.root.destroy()


def main():
    root = tk.Tk()
    App(root)
    root.mainloop()


if __name__ == "__main__":
    main()
