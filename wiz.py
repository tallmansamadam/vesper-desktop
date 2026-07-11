"""
wiz.py - WiZ (Philips/Signify) provider for the multi-vendor controller.

WiZ bulbs are bridge-free Wi-Fi devices controlled with a simple JSON-over-UDP
protocol on port 38899 - much like LIFX, so real-time effects/streaming work.

UNTESTED against real hardware (built to the documented setPilot/getPilot API).
Exposes the same method surface as LifxLan: discover / set_power / set_color /
set_brightness / set_white / stream_color / refresh.
"""

import colorsys
import json
import select
import socket
import time

WIZ_PORT = 38899


class WizLight:
    vendor = "WiZ"

    def __init__(self, ip, mac, label="", power=False, hsbk=None):
        self.ip = ip
        self.target = f"wiz:{mac}"
        self.mac = mac
        self.label = label or f"WiZ {mac[-6:]}"
        self.power = power
        self.hsbk = hsbk or (0, 0, 0, 3500)
        self.provider = None

    @property
    def serial(self):
        return self.mac

    @property
    def brightness_pct(self):
        return round(self.hsbk[2] / 65535 * 100)


def _state_to_hsbk(res):
    dim = res.get("dimming", 100)
    bri = int(max(0, min(100, dim)) / 100 * 65535)
    if res.get("temp"):
        return [0, 0, bri, int(res["temp"])]
    r, g, b = res.get("r", 0), res.get("g", 0), res.get("b", 0)
    if r or g or b:
        h, s, _v = colorsys.rgb_to_hsv(r / 255, g / 255, b / 255)
        return [int(h * 65535), int(s * 65535), bri, 3500]
    return [0, 0, bri, 3500]


class WizProvider:
    def __init__(self, timeout=0.5):
        self._cmd = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self._cmd.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        self._cmd.settimeout(timeout)

    def close(self):
        try:
            self._cmd.close()
        except OSError:
            pass

    # -- discovery (per-interface broadcast, VPN-aware) --------------------

    @staticmethod
    def _local_ipv4s():
        ips = set()
        try:
            for info in socket.getaddrinfo(socket.gethostname(), None,
                                           socket.AF_INET):
                ips.add(info[4][0])
        except OSError:
            pass
        return [ip for ip in ips if not ip.startswith("127.")]

    def discover(self, wait=1.2, rounds=2):
        payload = json.dumps({"method": "getPilot", "params": {}}).encode()
        socks = []
        for ip in self._local_ipv4s():
            try:
                s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
                s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                s.bind((ip, 0))
            except OSError:
                continue
            directed = ".".join(ip.split(".")[:3] + ["255"])
            socks.append((s, [directed, "255.255.255.255"]))
        if not socks:
            s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
            s.bind(("", 0))
            socks.append((s, ["255.255.255.255"]))

        found = {}
        readers = [s for s, _ in socks]
        try:
            for _ in range(rounds):
                for s, dests in socks:
                    for d in dests:
                        try:
                            s.sendto(payload, (d, WIZ_PORT))
                        except OSError:
                            pass
                deadline = time.time() + wait
                while time.time() < deadline:
                    ready, _, _ = select.select(readers, [], [], 0.2)
                    for s in ready:
                        try:
                            data, addr = s.recvfrom(2048)
                        except OSError:
                            continue
                        try:
                            res = json.loads(data).get("result", {})
                        except ValueError:
                            continue
                        mac = res.get("mac")
                        if mac and addr[0] not in found:
                            light = WizLight(addr[0], mac,
                                             power=res.get("state", False),
                                             hsbk=_state_to_hsbk(res))
                            light.provider = self
                            found[addr[0]] = light
        finally:
            for s, _ in socks:
                try:
                    s.close()
                except OSError:
                    pass
        return list(found.values())

    # -- commands ----------------------------------------------------------

    def _send(self, light, params):
        msg = json.dumps({"method": "setPilot", "params": params}).encode()
        try:
            self._cmd.sendto(msg, (light.ip, WIZ_PORT))
        except OSError:
            pass

    def refresh(self, light):
        msg = json.dumps({"method": "getPilot", "params": {}}).encode()
        try:
            self._cmd.sendto(msg, (light.ip, WIZ_PORT))
            data, _ = self._cmd.recvfrom(2048)
            res = json.loads(data).get("result", {})
            light.power = res.get("state", light.power)
            light.hsbk = _state_to_hsbk(res)
        except (OSError, ValueError):
            pass

    def set_power(self, light, on, duration_ms=0):
        self._send(light, {"state": bool(on)})
        light.power = bool(on)

    def set_color(self, light, hue, sat, bri, kelvin, duration_ms=0):
        pct = round(bri / 65535 * 100)
        if pct <= 0:
            self._send(light, {"state": False})
            light.hsbk = (hue & 0xFFFF, sat & 0xFFFF, 0, kelvin)
            return
        params = {"state": True, "dimming": max(10, min(100, pct))}
        if sat <= 300:                       # white
            params["temp"] = max(2200, min(6500, int(kelvin)))
        else:
            r, g, b = colorsys.hsv_to_rgb(hue / 65535, sat / 65535, 1.0)
            params.update({"r": int(r * 255), "g": int(g * 255),
                           "b": int(b * 255), "c": 0, "w": 0})
        self._send(light, params)
        light.hsbk = (hue & 0xFFFF, sat & 0xFFFF, bri & 0xFFFF, kelvin)

    def set_brightness(self, light, pct, duration_ms=0):
        pct = max(0, min(100, pct))
        if pct <= 0:
            self._send(light, {"state": False})
        else:
            self._send(light, {"state": True, "dimming": max(10, pct)})
        h, s, _b, k = light.hsbk
        light.hsbk = (h, s, int(pct / 100 * 65535), k)

    def set_white(self, light, pct, kelvin, duration_ms=0):
        self._send(light, {"state": True, "dimming": max(10, min(100, pct)),
                           "temp": max(2200, min(6500, int(kelvin)))})
        light.hsbk = (0, 0, int(pct / 100 * 65535), kelvin)

    def stream_color(self, light, hue, sat, bri, kelvin, duration_ms=0):
        self.set_color(light, hue, sat, bri, kelvin)
