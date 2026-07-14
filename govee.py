"""
govee.py - Govee provider for the multi-vendor controller.

Newer Govee lights support a local LAN API (no cloud, no API key) once
"LAN Control" is toggled on for the device in the Govee Home app. The
protocol is JSON-over-UDP:

  scan     -> multicast 239.255.255.250:4001, devices reply to OUR ip:4002
  control  -> unicast   <device>:4003 (turn / brightness / colorwc)
  status   -> devStatus to <device>:4003, reply again lands on our :4002

Because replies always target port 4002 specifically, the provider binds one
listener socket to that port for its lifetime. If another process owns 4002,
Govee support degrades gracefully to "no devices found".

UNTESTED against real hardware (built to Govee's published LAN API doc).
"""

import colorsys
import json
import select
import socket
import time

GOVEE_MCAST = ("239.255.255.250", 4001)
GOVEE_RESP_PORT = 4002
GOVEE_CTRL_PORT = 4003


def _local_ipv4s():
    ips = set()
    try:
        for info in socket.getaddrinfo(socket.gethostname(), None,
                                       socket.AF_INET):
            ips.add(info[4][0])
    except OSError:
        pass
    return [ip for ip in ips if not ip.startswith("127.")]


def parse_msg(data):
    """Decode a LAN-API datagram -> (cmd, data dict) or (None, {})."""
    try:
        msg = json.loads(data).get("msg", {})
        return msg.get("cmd"), msg.get("data", {}) or {}
    except (ValueError, AttributeError):
        return None, {}


def status_to_hsbk(d):
    """devStatus data -> [hue, sat, bri, kelvin]."""
    bri = int(max(0, min(100, d.get("brightness", 100))) / 100 * 65535)
    kelvin = int(d.get("colorTemInKelvin") or 0)
    if kelvin:
        return [0, 0, bri, kelvin]
    c = d.get("color", {}) or {}
    r, g, b = c.get("r", 0), c.get("g", 0), c.get("b", 0)
    if r or g or b:
        h, s, _v = colorsys.rgb_to_hsv(r / 255, g / 255, b / 255)
        return [int(h * 65535), int(s * 65535), bri, 3500]
    return [0, 0, bri, 3500]


class GoveeLight:
    vendor = "Govee"

    def __init__(self, ip, device, sku="", power=False, hsbk=None):
        self.ip = ip
        self.target = f"govee:{device}"
        self.device = device               # MAC-style id from the scan reply
        self.label = f"Govee {sku}".strip() if sku else "Govee"
        self.power = power
        self.hsbk = hsbk or [0, 0, 0, 3500]
        self.provider = None

    @property
    def serial(self):
        return self.device.replace(":", "")[-12:]

    @property
    def brightness_pct(self):
        return round(self.hsbk[2] / 65535 * 100)


class GoveeProvider:
    def __init__(self):
        # Replies are always sent to our ip:4002, so hold that port open.
        try:
            rx = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            rx.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            rx.bind(("", GOVEE_RESP_PORT))
            rx.setblocking(False)
            self._rx = rx
        except OSError:
            self._rx = None                # port taken; Govee disabled
        self._tx = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)

    def close(self):
        for s in (self._rx, self._tx):
            try:
                if s:
                    s.close()
            except OSError:
                pass

    # -- plumbing -------------------------------------------------------------

    def _send(self, ip_port, cmd, data):
        payload = json.dumps({"msg": {"cmd": cmd, "data": data}}).encode()
        try:
            self._tx.sendto(payload, ip_port)
        except OSError:
            pass

    def _drain(self):
        if not self._rx:
            return
        while True:
            try:
                self._rx.recvfrom(4096)
            except (BlockingIOError, OSError):
                return

    def _collect(self, wait, handler):
        """Feed (cmd, data, source-ip) replies to handler until the deadline."""
        deadline = time.time() + wait
        while time.time() < deadline:
            ready, _, _ = select.select([self._rx], [], [], 0.2)
            if not ready:
                continue
            try:
                data, addr = self._rx.recvfrom(4096)
            except OSError:
                return
            cmd, d = parse_msg(data)
            if cmd:
                handler(cmd, d, addr[0])

    # -- provider API -----------------------------------------------------------

    def discover(self, wait=1.5):
        if not self._rx:
            return []
        self._drain()
        scan = json.dumps(
            {"msg": {"cmd": "scan", "data": {"account_topic": "reserve"}}}
        ).encode()
        sent = False
        for ip in _local_ipv4s():
            try:
                s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                s.bind((ip, 0))
                s.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF,
                             socket.inet_aton(ip))
                s.sendto(scan, GOVEE_MCAST)
                s.close()
                sent = True
            except OSError:
                continue
        if not sent:
            self._send(GOVEE_MCAST, "scan", {"account_topic": "reserve"})

        found = {}

        def on_reply(cmd, d, src_ip):
            if cmd == "scan" and d.get("device"):
                light = GoveeLight(d.get("ip") or src_ip, d["device"],
                                   d.get("sku", ""))
                light.provider = self
                found[d["device"]] = light

        self._collect(wait, on_reply)

        lights = list(found.values())
        for light in lights:
            self.refresh(light)
        return lights

    def refresh(self, light):
        if not self._rx:
            return
        self._drain()
        self._send((light.ip, GOVEE_CTRL_PORT), "devStatus", {})
        state = {}

        def on_reply(cmd, d, src_ip):
            if cmd == "devStatus" and src_ip == light.ip:
                state.update(d)

        self._collect(0.8, on_reply)
        if state:
            light.power = bool(state.get("onOff", 0))
            light.hsbk = status_to_hsbk(state)

    def set_power(self, light, on, duration_ms=0):
        self._send((light.ip, GOVEE_CTRL_PORT), "turn", {"value": 1 if on else 0})
        light.power = bool(on)

    def set_color(self, light, hue, sat, bri, kelvin, duration_ms=0):
        pct = max(1, round(bri / 65535 * 100))
        if sat <= 300:
            self.set_white(light, pct, kelvin)
            return
        r, g, b = colorsys.hsv_to_rgb(hue / 65535, sat / 65535, 1.0)
        self._send((light.ip, GOVEE_CTRL_PORT), "colorwc", {
            "color": {"r": int(r * 255), "g": int(g * 255), "b": int(b * 255)},
            "colorTemInKelvin": 0,
        })
        self._send((light.ip, GOVEE_CTRL_PORT), "brightness", {"value": pct})
        light.hsbk = [hue & 0xFFFF, sat & 0xFFFF, bri & 0xFFFF, light.hsbk[3]]

    def set_brightness(self, light, pct, duration_ms=0):
        pct = max(1, min(100, int(pct)))
        self._send((light.ip, GOVEE_CTRL_PORT), "brightness", {"value": pct})
        h, s, _b, k = light.hsbk
        light.hsbk = [h, s, int(pct / 100 * 65535), k]

    def set_white(self, light, pct, kelvin, duration_ms=0):
        kelvin = max(2000, min(9000, int(kelvin)))
        self._send((light.ip, GOVEE_CTRL_PORT), "colorwc",
                   {"color": {"r": 0, "g": 0, "b": 0},
                    "colorTemInKelvin": kelvin})
        self._send((light.ip, GOVEE_CTRL_PORT), "brightness",
                   {"value": max(1, min(100, int(pct)))})
        light.hsbk = [0, 0, int(pct / 100 * 65535), kelvin]

    def stream_color(self, light, hue, sat, bri, kelvin, duration_ms=0):
        # UDP fire-and-forget; cheap enough for effect frame rates.
        self.set_color(light, hue, sat, bri, kelvin)
