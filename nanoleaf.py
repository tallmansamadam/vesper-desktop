"""
nanoleaf.py - Nanoleaf provider for the multi-vendor controller.

Nanoleaf panels (Aurora/Canvas/Shapes/...) expose a local REST API on port
16021. Pairing works like Hue's link button: hold the panel's power button
5-7 seconds until the LEDs flash, then POST /api/v1/new to mint an auth
token. Tokens are persisted in nanoleaf.json (gitignored - credentials).

Discovery is SSDP: an M-SEARCH on 239.255.255.250:1900, filtering replies
that identify as Nanoleaf devices.

UNTESTED against real hardware (built to the documented Open API). Exposes
the same surface as the other providers.
"""

import json
import os
import select
import socket
import time
import urllib.request

_SETTINGS = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                         "nanoleaf.json")
SSDP_MCAST = ("239.255.255.250", 1900)
_MSEARCH = ("M-SEARCH * HTTP/1.1\r\n"
            "HOST: 239.255.255.250:1900\r\n"
            'MAN: "ssdp:discover"\r\n'
            "MX: 2\r\n"
            "ST: ssdp:all\r\n\r\n").encode()

_STREAM_MIN_INTERVAL = 0.3


def _local_ipv4s():
    ips = set()
    try:
        for info in socket.getaddrinfo(socket.gethostname(), None,
                                       socket.AF_INET):
            ips.add(info[4][0])
    except OSError:
        pass
    return [ip for ip in ips if not ip.startswith("127.")]


def parse_ssdp_response(text):
    """Parse SSDP reply headers into a lowercase-keyed dict."""
    headers = {}
    for line in text.split("\r\n"):
        if ":" not in line:
            continue
        key, _, value = line.partition(":")
        headers[key.strip().lower()] = value.strip()
    return headers


def is_nanoleaf(headers):
    blob = " ".join(headers.get(k, "") for k in ("st", "usn", "server",
                                                 "nl-devicename")).lower()
    return "nanoleaf" in blob or "aurora" in blob


def location_hostport(headers):
    """(ip, port) from an SSDP Location header, or (None, None)."""
    loc = headers.get("location", "")
    if not loc.startswith("http://"):
        return None, None
    hostport = loc[len("http://"):].split("/", 1)[0]
    ip, _, p = hostport.partition(":")
    try:
        return ip, int(p or 16021)
    except ValueError:
        return ip, 16021


def state_to_hsbk(info):
    """Panel /state json -> [hue, sat, bri, kelvin]."""
    st = info.get("state", {})

    def val(key, default):
        v = st.get(key, {})
        return v.get("value", default) if isinstance(v, dict) else default

    bri = int(val("brightness", 100) / 100 * 65535)
    if st.get("colorMode") == "ct":
        return [0, 0, bri, int(val("ct", 4000))]
    hue = int(val("hue", 0) / 360 * 65535)
    sat = int(val("sat", 0) / 100 * 65535)
    return [hue, sat, bri, 3500]


class NanoleafLight:
    vendor = "Nanoleaf"

    def __init__(self, ip, port, label="", power=False, hsbk=None):
        self.ip = ip
        self.port = port
        self.target = f"nanoleaf:{ip}"
        self.label = label or "Nanoleaf"
        self.power = power
        self.hsbk = hsbk or [0, 0, 0, 3500]
        self.provider = None

    @property
    def serial(self):
        return self.ip

    @property
    def brightness_pct(self):
        return round(self.hsbk[2] / 65535 * 100)


class NanoleafProvider:
    def __init__(self, devices=None):
        # {ip: {"port": 16021, "token": "..."}}
        self.devices = devices or {}
        self._last_stream = {}

    @classmethod
    def from_settings(cls):
        try:
            with open(_SETTINGS, encoding="utf-8") as fh:
                return cls(json.load(fh))
        except (OSError, ValueError):
            return cls()

    def _save(self):
        with open(_SETTINGS, "w", encoding="utf-8") as fh:
            json.dump(self.devices, fh, indent=2)

    def close(self):
        pass

    # -- SSDP search + pairing ------------------------------------------------

    @staticmethod
    def ssdp_search(wait=1.8):
        """Return {ip: port} of Nanoleaf devices answering SSDP."""
        socks = []
        for ip in _local_ipv4s():
            try:
                s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                s.bind((ip, 0))
                s.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF,
                             socket.inet_aton(ip))
                s.sendto(_MSEARCH, SSDP_MCAST)
                socks.append(s)
            except OSError:
                continue
        found = {}
        deadline = time.time() + wait
        try:
            while time.time() < deadline:
                ready, _, _ = select.select(socks, [], [], 0.2)
                for s in ready:
                    try:
                        data, addr = s.recvfrom(4096)
                    except OSError:
                        continue
                    h = parse_ssdp_response(data.decode(errors="replace"))
                    if not is_nanoleaf(h):
                        continue
                    ip, port = location_hostport(h)
                    found[ip or addr[0]] = port or 16021
        finally:
            for s in socks:
                try:
                    s.close()
                except OSError:
                    pass
        return found

    def pair(self, attempts=25, on_wait=None):
        """Mint tokens for un-paired panels. Hold the power button 5-7s until
        the LEDs flash, then call this. Returns how many devices paired."""
        candidates = self.ssdp_search()
        fresh = {ip: port for ip, port in candidates.items()
                 if ip not in self.devices}
        if not fresh:
            return 0
        paired = 0
        for i in range(attempts):
            for ip, port in list(fresh.items()):
                try:
                    req = urllib.request.Request(
                        f"http://{ip}:{port}/api/v1/new", data=b"",
                        method="POST")
                    with urllib.request.urlopen(req, timeout=4) as r:
                        token = json.loads(r.read()).get("auth_token")
                    if token:
                        self.devices[ip] = {"port": port, "token": token}
                        fresh.pop(ip)
                        paired += 1
                        self._save()
                except Exception:
                    pass                       # not in pairing mode yet
            if not fresh:
                break
            if on_wait:
                on_wait(attempts - i)
            time.sleep(1)
        return paired

    # -- REST helpers -----------------------------------------------------------

    def _url(self, light):
        d = self.devices.get(light.ip, {})
        return (f"http://{light.ip}:{d.get('port', light.port)}"
                f"/api/v1/{d.get('token', '')}")

    def _get(self, light, path=""):
        with urllib.request.urlopen(self._url(light) + path, timeout=4) as r:
            return json.loads(r.read())

    def _put_state(self, light, payload):
        try:
            req = urllib.request.Request(
                self._url(light) + "/state",
                data=json.dumps(payload).encode(), method="PUT",
                headers={"Content-Type": "application/json"})
            urllib.request.urlopen(req, timeout=4).close()
        except Exception:
            pass

    # -- provider API -------------------------------------------------------------

    def discover(self):
        lights = []
        for ip, d in self.devices.items():
            light = NanoleafLight(ip, d.get("port", 16021))
            try:
                info = self._get(light)
            except Exception:
                continue                        # offline right now
            light.label = info.get("name", "Nanoleaf")
            light.power = bool(
                info.get("state", {}).get("on", {}).get("value", False))
            light.hsbk = state_to_hsbk(info)
            light.provider = self
            lights.append(light)
        return lights

    def refresh(self, light):
        try:
            info = self._get(light)
        except Exception:
            return
        light.power = bool(
            info.get("state", {}).get("on", {}).get("value", False))
        light.hsbk = state_to_hsbk(info)

    def set_power(self, light, on, duration_ms=400):
        self._put_state(light, {"on": {"value": bool(on)}})
        light.power = bool(on)

    def set_color(self, light, hue, sat, bri, kelvin, duration_ms=400):
        pct = max(1, round(bri / 65535 * 100))
        if sat <= 300:
            self.set_white(light, pct, kelvin, duration_ms=duration_ms)
            return
        self._put_state(light, {
            "on": {"value": True},
            "hue": {"value": round(hue / 65535 * 360) % 360},
            "sat": {"value": round(sat / 65535 * 100)},
            "brightness": {"value": pct},
        })
        light.power = True
        light.hsbk = [hue & 0xFFFF, sat & 0xFFFF, bri & 0xFFFF, light.hsbk[3]]

    def set_brightness(self, light, pct, duration_ms=300):
        pct = max(1, min(100, int(pct)))
        self._put_state(light, {"brightness": {"value": pct}})
        h, s, _b, k = light.hsbk
        light.hsbk = [h, s, int(pct / 100 * 65535), k]

    def set_white(self, light, pct, kelvin, duration_ms=400):
        kelvin = max(1200, min(6500, int(kelvin)))
        pct = max(1, min(100, int(pct)))
        self._put_state(light, {
            "on": {"value": True},
            "ct": {"value": kelvin},
            "brightness": {"value": pct},
        })
        light.power = True
        light.hsbk = [0, 0, int(pct / 100 * 65535), kelvin]

    def stream_color(self, light, hue, sat, bri, kelvin, duration_ms=250):
        # REST per frame is heavy; throttle. (Full-rate would use Nanoleaf's
        # external-control UDP streaming - out of scope.)
        now = time.monotonic()
        if now - self._last_stream.get(light.target, 0.0) < _STREAM_MIN_INTERVAL:
            return
        self._last_stream[light.target] = now
        self.set_color(light, hue, sat, bri, kelvin, duration_ms=duration_ms)
