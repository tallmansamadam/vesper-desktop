"""
hue.py - Philips Hue provider for the multi-vendor controller.

Hue bulbs are Zigbee behind a Hue Bridge; there is no per-bulb LAN protocol.
Control is HTTPS REST to the bridge (CLIP v2). Setup requires pressing the
bridge's physical link button once to obtain an application key.

UNTESTED against real hardware (built to the documented CLIP v2 API). Exposes the
same method surface as LifxLan so the app's controller can treat it uniformly:
discover / set_power / set_color / set_brightness / set_white / stream_color /
refresh. Colour is HSBK in, converted to CIE-xy for the bridge.

Docs: https://developers.meethue.com/develop/hue-api-v2/
"""

import colorsys
import json
import os
import ssl
import time
import urllib.request

_SETTINGS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "hue.json")
_APP_NAME = "lifx_control#desktop"
_SSL = ssl._create_unverified_context()          # bridge uses a self-signed cert


# --------------------------------------------------------------------------
# Colour conversion  (HSBK <-> CIE xy, per Hue's recommended transform)
# --------------------------------------------------------------------------

def _gamma(c):
    return ((c + 0.055) / 1.055) ** 2.4 if c > 0.04045 else c / 12.92


def _ungamma(c):
    return 1.055 * (c ** (1 / 2.4)) - 0.055 if c > 0.0031308 else 12.92 * c


def hsbk_to_xy(hue, sat, bri):
    r, g, b = colorsys.hsv_to_rgb(hue / 65535, sat / 65535, 1.0)
    r, g, b = _gamma(r), _gamma(g), _gamma(b)
    x = r * 0.664511 + g * 0.154324 + b * 0.162028
    y = r * 0.283881 + g * 0.668433 + b * 0.047685
    z = r * 0.000088 + g * 0.072310 + b * 0.986039
    total = x + y + z or 1.0
    return [round(x / total, 4), round(y / total, 4)]


def xy_to_hsbk(x, y, bri_pct, kelvin=3500):
    if y <= 0:
        y = 0.0001
    yy = 1.0
    xx = (yy / y) * x
    zz = (yy / y) * (1 - x - y)
    r = xx * 1.656492 - yy * 0.354851 - zz * 0.255038
    g = -xx * 0.707196 + yy * 1.655397 + zz * 0.036152
    b = xx * 0.051713 - yy * 0.121364 + zz * 1.011530
    r, g, b = (max(0.0, min(1.0, _ungamma(v))) for v in (r, g, b))
    h, s, _v = colorsys.rgb_to_hsv(r, g, b)
    bri = int(max(0, min(100, bri_pct)) / 100 * 65535)
    return [int(h * 65535), int(s * 65535), bri, kelvin]


# --------------------------------------------------------------------------
# Light + provider
# --------------------------------------------------------------------------

class HueLight:
    vendor = "Hue"

    def __init__(self, rid, ip, label="", power=False, hsbk=None):
        self.target = f"hue:{rid}"          # hashable id for the app
        self.rid = rid                      # CLIP v2 resource id
        self.ip = ip                        # bridge ip (shown in the card)
        self.label = label
        self.power = power
        self.hsbk = hsbk or (0, 0, 0, 3500)
        self.provider = None

    @property
    def serial(self):
        return self.rid[:12]

    @property
    def brightness_pct(self):
        return round(self.hsbk[2] / 65535 * 100)


class HueProvider:
    """One Hue bridge. Loads/saves its credentials in hue.json."""

    def __init__(self, ip=None, app_key=None):
        self.ip = ip
        self.app_key = app_key

    # -- persistence -------------------------------------------------------

    @classmethod
    def from_settings(cls):
        try:
            with open(_SETTINGS, encoding="utf-8") as fh:
                d = json.load(fh)
            if d.get("ip") and d.get("app_key"):
                return cls(d["ip"], d["app_key"])
        except (OSError, ValueError):
            pass
        return None

    def _save(self):
        with open(_SETTINGS, "w", encoding="utf-8") as fh:
            json.dump({"ip": self.ip, "app_key": self.app_key}, fh, indent=2)

    # -- discovery + pairing ----------------------------------------------

    @staticmethod
    def find_bridge_ip():
        """Locate a bridge IP via Hue's discovery service (needs internet)."""
        try:
            req = urllib.request.Request(
                "https://discovery.meethue.com/",
                headers={"User-Agent": "lifx-control"})
            with urllib.request.urlopen(req, timeout=8) as r:
                data = json.loads(r.read())
            if data:
                return data[0].get("internalipaddress")
        except Exception:
            return None
        return None

    def pair(self, ip=None, attempts=30, on_wait=None):
        """Create an app key. Press the bridge link button, then call this.

        Retries for ~`attempts` seconds waiting for the button press. Returns
        True on success. `on_wait(remaining)` is called each second if given.
        """
        self.ip = ip or self.ip or self.find_bridge_ip()
        if not self.ip:
            raise RuntimeError("No Hue bridge found on the network.")
        body = json.dumps(
            {"devicetype": _APP_NAME, "generateclientkey": True}).encode()
        for i in range(attempts):
            try:
                req = urllib.request.Request(
                    f"https://{self.ip}/api", data=body, method="POST",
                    headers={"Content-Type": "application/json"})
                with urllib.request.urlopen(req, timeout=8, context=_SSL) as r:
                    res = json.loads(r.read())
                if res and "success" in res[0]:
                    self.app_key = res[0]["success"]["username"]
                    self._save()
                    return True
            except Exception:
                pass
            if on_wait:
                on_wait(attempts - i)
            time.sleep(1)
        return False

    # -- REST helpers ------------------------------------------------------

    def _req(self, method, path, payload=None):
        url = f"https://{self.ip}/clip/v2/resource/{path}"
        data = json.dumps(payload).encode() if payload is not None else None
        req = urllib.request.Request(
            url, data=data, method=method,
            headers={"hue-application-key": self.app_key,
                     "Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=6, context=_SSL) as r:
            return json.loads(r.read())

    # -- provider API ------------------------------------------------------

    def discover(self):
        if not (self.ip and self.app_key):
            return []
        lights = []
        try:
            data = self._req("GET", "light").get("data", [])
        except Exception:
            return []
        for it in data:
            light = HueLight(it["id"], self.ip,
                             label=it.get("metadata", {}).get("name", "Hue"))
            self._apply_state(light, it)
            light.provider = self
            lights.append(light)
        return lights

    def _apply_state(self, light, it):
        light.power = it.get("on", {}).get("on", False)
        bri = it.get("dimming", {}).get("brightness", 100)
        xy = it.get("color", {}).get("xy")
        if xy:
            light.hsbk = xy_to_hsbk(xy["x"], xy["y"], bri)
        else:
            mirek = it.get("color_temperature", {}).get("mirek") or 250
            kelvin = int(1_000_000 / mirek)
            light.hsbk = [0, 0, int(bri / 100 * 65535), kelvin]

    def refresh(self, light):
        try:
            data = self._req("GET", f"light/{light.rid}").get("data", [])
            if data:
                self._apply_state(light, data[0])
        except Exception:
            pass

    def _put(self, light, payload):
        try:
            self._req("PUT", f"light/{light.rid}", payload)
        except Exception:
            pass

    def set_power(self, light, on, duration_ms=400):
        self._put(light, {"on": {"on": bool(on)}})
        light.power = bool(on)

    def set_color(self, light, hue, sat, bri, kelvin, duration_ms=400):
        pct = round(bri / 65535 * 100)
        payload = {"on": {"on": True},
                   "dimming": {"brightness": max(1, pct)},
                   "dynamics": {"duration": int(duration_ms)}}
        if sat <= 300:                       # treat as white
            mirek = max(153, min(500, round(1_000_000 / max(1, kelvin))))
            payload["color_temperature"] = {"mirek": mirek}
        else:
            x, y = hsbk_to_xy(hue, sat, bri)
            payload["color"] = {"xy": {"x": x, "y": y}}
        self._put(light, payload)
        light.hsbk = (hue & 0xFFFF, sat & 0xFFFF, bri & 0xFFFF, kelvin)

    def set_brightness(self, light, pct, duration_ms=300):
        self._put(light, {"on": {"on": True},
                          "dimming": {"brightness": max(1, min(100, pct))},
                          "dynamics": {"duration": int(duration_ms)}})
        h, s, _b, k = light.hsbk
        light.hsbk = (h, s, int(pct / 100 * 65535), k)

    def set_white(self, light, pct, kelvin, duration_ms=400):
        mirek = max(153, min(500, round(1_000_000 / max(1, kelvin))))
        self._put(light, {"on": {"on": True},
                          "dimming": {"brightness": max(1, min(100, pct))},
                          "color_temperature": {"mirek": mirek},
                          "dynamics": {"duration": int(duration_ms)}})
        light.hsbk = (0, 0, int(pct / 100 * 65535), kelvin)

    def stream_color(self, light, hue, sat, bri, kelvin, duration_ms=250):
        # NOTE: the bridge rate-limits REST writes (~10/s); real-time effects and
        # screen-sync are best-effort on Hue and ideally use the Entertainment
        # (DTLS) API, which is out of scope here.
        self.set_color(light, hue, sat, bri, kelvin, duration_ms=duration_ms)

    def close(self):
        pass
