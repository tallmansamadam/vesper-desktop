"""
yeelight.py - Yeelight provider for the multi-vendor controller.

Yeelight Wi-Fi bulbs speak JSON-over-TCP on port 55443 once "LAN Control" is
enabled for the bulb in the Yeelight app. Discovery is an SSDP-style M-SEARCH
on multicast 239.255.255.250:1982, answered unicast with a header block
describing the bulb's state.

UNTESTED against real hardware (built to the documented Yeelight
Inter-Operation spec). Exposes the same surface as the other providers:
discover / set_power / set_color / set_brightness / set_white / stream_color /
refresh / close.

Note: the LAN interface enforces a quota (~60 commands/minute per bulb), so
stream_color is throttled per light; full-rate effects would need Yeelight's
"music mode" (out of scope).
"""

import colorsys
import json
import select
import socket
import threading
import time

YEELIGHT_MCAST = ("239.255.255.250", 1982)
_MSEARCH = ("M-SEARCH * HTTP/1.1\r\n"
            "HOST: 239.255.255.250:1982\r\n"
            'MAN: "ssdp:discover"\r\n'
            "ST: wifi_bulb\r\n\r\n").encode()

_STREAM_MIN_INTERVAL = 0.4          # seconds between streamed frames per light


def _local_ipv4s():
    ips = set()
    try:
        for info in socket.getaddrinfo(socket.gethostname(), None,
                                       socket.AF_INET):
            ips.add(info[4][0])
    except OSError:
        pass
    return [ip for ip in ips if not ip.startswith("127.")]


def parse_search_response(text):
    """Parse an M-SEARCH reply's headers into a dict (keys lowercased)."""
    headers = {}
    for line in text.split("\r\n"):
        if ":" not in line:
            continue
        key, _, value = line.partition(":")
        headers[key.strip().lower()] = value.strip()
    return headers


def headers_to_state(h):
    """(ip, port, ident, label, power, hsbk) from parsed discovery headers."""
    loc = h.get("location", "")            # yeelight://192.168.x.x:55443
    ip = port = None
    if loc.startswith("yeelight://"):
        hostport = loc[len("yeelight://"):]
        ip, _, p = hostport.partition(":")
        port = int(p or 55443)
    ident = h.get("id", ip or "")
    label = h.get("name", "") or f"Yeelight {h.get('model', '')}".strip()
    power = h.get("power", "off") == "on"
    bri = int(h.get("bright", 100) or 100)
    b16 = int(bri / 100 * 65535)
    mode = h.get("color_mode", "2")
    if mode == "1":                        # rgb
        rgb = int(h.get("rgb", 0) or 0)
        r, g, b = (rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF
        hh, ss, _v = colorsys.rgb_to_hsv(r / 255, g / 255, b / 255)
        hsbk = [int(hh * 65535), int(ss * 65535), b16, 3500]
    elif mode == "3":                      # hsv
        hue = int(h.get("hue", 0) or 0)
        sat = int(h.get("sat", 0) or 0)
        hsbk = [int(hue / 360 * 65535), int(sat / 100 * 65535), b16, 3500]
    else:                                  # ct
        kelvin = int(h.get("ct", 4000) or 4000)
        hsbk = [0, 0, b16, kelvin]
    return ip, port, ident, label, power, hsbk


class YeelightLight:
    vendor = "Yeelight"

    def __init__(self, ip, port, ident, label="", power=False, hsbk=None):
        self.ip = ip
        self.port = port or 55443
        self.target = f"yeelight:{ident}"
        self.ident = ident
        self.label = label or "Yeelight"
        self.power = power
        self.hsbk = hsbk or [0, 0, 0, 3500]
        self.provider = None

    @property
    def serial(self):
        return str(self.ident)[-12:]

    @property
    def brightness_pct(self):
        return round(self.hsbk[2] / 65535 * 100)


class YeelightProvider:
    def __init__(self):
        self._conns = {}                   # target -> socket
        self._lock = threading.Lock()
        self._msg_id = 0
        self._last_stream = {}             # target -> monotonic seconds

    def close(self):
        with self._lock:
            for s in self._conns.values():
                try:
                    s.close()
                except OSError:
                    pass
            self._conns.clear()

    # -- discovery ----------------------------------------------------------

    def discover(self, wait=1.5):
        socks = []
        for ip in _local_ipv4s():
            try:
                s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                s.bind((ip, 0))
                s.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF,
                             socket.inet_aton(ip))
                s.sendto(_MSEARCH, YEELIGHT_MCAST)
                socks.append(s)
            except OSError:
                continue
        if not socks:
            try:
                s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                s.sendto(_MSEARCH, YEELIGHT_MCAST)
                socks.append(s)
            except OSError:
                return []

        found = {}
        deadline = time.time() + wait
        try:
            while time.time() < deadline:
                ready, _, _ = select.select(socks, [], [], 0.2)
                for s in ready:
                    try:
                        data, _addr = s.recvfrom(2048)
                    except OSError:
                        continue
                    h = parse_search_response(data.decode(errors="replace"))
                    ip, port, ident, label, power, hsbk = headers_to_state(h)
                    if ip and ident not in found:
                        light = YeelightLight(ip, port, ident, label,
                                              power, hsbk)
                        light.provider = self
                        found[ident] = light
        finally:
            for s in socks:
                try:
                    s.close()
                except OSError:
                    pass
        return list(found.values())

    # -- TCP command channel --------------------------------------------------

    def _cmd(self, light, method, params, want_result=False):
        """Send one command; returns the parsed result list or None."""
        with self._lock:
            for attempt in range(2):
                sock = self._conns.get(light.target)
                try:
                    if sock is None:
                        sock = socket.create_connection(
                            (light.ip, light.port), timeout=2)
                        sock.settimeout(1.5)
                        self._conns[light.target] = sock
                    self._msg_id += 1
                    msg_id = self._msg_id
                    payload = json.dumps({"id": msg_id, "method": method,
                                          "params": params}) + "\r\n"
                    sock.sendall(payload.encode())
                    return self._read_reply(sock, msg_id, want_result)
                except (OSError, ValueError):
                    self._conns.pop(light.target, None)
                    try:
                        if sock:
                            sock.close()
                    except OSError:
                        pass
                    sock = None
                    if attempt:
                        return None
        return None

    @staticmethod
    def _read_reply(sock, msg_id, want_result):
        """Read reply lines, skipping unsolicited notifications."""
        buf = b""
        deadline = time.time() + 1.5
        while time.time() < deadline:
            try:
                chunk = sock.recv(4096)
            except socket.timeout:
                return None
            if not chunk:
                raise OSError("connection closed")
            buf += chunk
            while b"\r\n" in buf:
                line, _, buf = buf.partition(b"\r\n")
                try:
                    obj = json.loads(line)
                except ValueError:
                    continue
                if obj.get("id") == msg_id:
                    return obj.get("result") if want_result else obj
        return None

    # -- provider API ---------------------------------------------------------

    def refresh(self, light):
        res = self._cmd(light, "get_prop",
                        ["power", "bright", "ct", "rgb", "hue", "sat",
                         "color_mode", "name"], want_result=True)
        if not res or len(res) < 7:
            return
        power, bright, ct, rgb, hue, sat, mode = res[:7]
        light.power = power == "on"
        b16 = int(int(bright or 100) / 100 * 65535)
        if mode == "1":
            v = int(rgb or 0)
            r, g, b = (v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF
            hh, ss, _ = colorsys.rgb_to_hsv(r / 255, g / 255, b / 255)
            light.hsbk = [int(hh * 65535), int(ss * 65535), b16, 3500]
        elif mode == "3":
            light.hsbk = [int(int(hue or 0) / 360 * 65535),
                          int(int(sat or 0) / 100 * 65535), b16, 3500]
        else:
            light.hsbk = [0, 0, b16, int(ct or 4000)]
        if len(res) >= 8 and res[7]:
            light.label = res[7]

    def set_power(self, light, on, duration_ms=400):
        self._cmd(light, "set_power",
                  ["on" if on else "off", "smooth", max(30, int(duration_ms))])
        light.power = bool(on)

    def set_color(self, light, hue, sat, bri, kelvin, duration_ms=400):
        pct = max(1, round(bri / 65535 * 100))
        if sat <= 300:                    # effectively white
            self.set_white(light, pct, kelvin, duration_ms=duration_ms)
            return
        r, g, b = colorsys.hsv_to_rgb(hue / 65535, sat / 65535, 1.0)
        rgb = (int(r * 255) << 16) | (int(g * 255) << 8) | int(b * 255)
        # set_scene sets colour + brightness atomically (and powers on)
        self._cmd(light, "set_scene", ["color", rgb, pct])
        light.power = True
        light.hsbk = [hue & 0xFFFF, sat & 0xFFFF, bri & 0xFFFF, light.hsbk[3]]

    def set_brightness(self, light, pct, duration_ms=300):
        pct = max(1, min(100, int(pct)))
        self._cmd(light, "set_bright", [pct, "smooth", max(30, int(duration_ms))])
        h, s, _b, k = light.hsbk
        light.hsbk = [h, s, int(pct / 100 * 65535), k]

    def set_white(self, light, pct, kelvin, duration_ms=400):
        kelvin = max(1700, min(6500, int(kelvin)))
        pct = max(1, min(100, int(pct)))
        self._cmd(light, "set_scene", ["ct", kelvin, pct])
        light.power = True
        light.hsbk = [0, 0, int(pct / 100 * 65535), kelvin]

    def stream_color(self, light, hue, sat, bri, kelvin, duration_ms=250):
        # Respect the LAN quota: drop frames arriving faster than the
        # per-light interval instead of flooding the bulb.
        now = time.monotonic()
        if now - self._last_stream.get(light.target, 0.0) < _STREAM_MIN_INTERVAL:
            return
        self._last_stream[light.target] = now
        self.set_color(light, hue, sat, bri, kelvin, duration_ms=duration_ms)
