"""
lifx_lan.py - Zero-dependency LIFX LAN protocol client.

Talks to LIFX bulbs directly over your local network using UDP (port 56700).
No cloud, no API token, no third-party libraries - just the Python stdlib.

Protocol reference: https://lan.developer.lifx.com/docs

Public API:
    lan = LifxLan()
    lights = lan.discover()            # -> list[Light]
    lan.set_power(light, True)         # on / off
    lan.set_brightness(light, 50)      # 0..100 percent
    lan.set_rgb(light, 255, 0, 0)      # color by RGB (0..255)
    lan.set_white(light, 50, 3500)     # brightness%, kelvin
    lan.refresh(light)                 # re-read live state into the Light
"""

import colorsys
import select
import socket
import struct
import time

LIFX_PORT = 56700
BROADCAST = "255.255.255.255"

# Message types we use
MSG_GET_SERVICE = 2
MSG_STATE_SERVICE = 3
MSG_GET = 101  # GetColor
MSG_STATE = 107  # State (HSBK + power + label)
MSG_SET_COLOR = 102
MSG_SET_POWER = 117  # light SetPower

KELVIN_MIN = 1500
KELVIN_MAX = 9000


# --------------------------------------------------------------------------
# Packet building / parsing
# --------------------------------------------------------------------------

def _make_packet(msg_type, payload=b"", target=0, source=2,
                 tagged=False, res_required=False, ack_required=False,
                 sequence=0):
    """Build a LIFX LAN packet (36-byte header + payload)."""
    size = 36 + len(payload)

    # Frame header (8 bytes): size, protocol+flags, source
    protocol = 1024
    flags = protocol | (1 << 12)          # addressable bit always set
    if tagged:
        flags |= (1 << 13)                # tagged = broadcast to all
    frame = struct.pack("<HHI", size, flags, source)

    # Frame address (16 bytes): target(8) + reserved(6) + res/ack(1) + seq(1)
    resack = (1 if res_required else 0) | ((1 if ack_required else 0) << 1)
    frame_address = struct.pack("<Q6xBB", target, resack, sequence)

    # Protocol header (12 bytes): reserved(8) + type(2) + reserved(2)
    proto_header = struct.pack("<QHH", 0, msg_type, 0)

    return frame + frame_address + proto_header + payload


def _parse_header(data):
    """Return (size, source, target, sequence, msg_type) from a packet."""
    size, _flags, source = struct.unpack_from("<HHI", data, 0)
    (target,) = struct.unpack_from("<Q", data, 8)
    sequence = data[23]
    (msg_type,) = struct.unpack_from("<H", data, 32)
    return size, source, target, sequence, msg_type


# --------------------------------------------------------------------------
# Color helpers
# --------------------------------------------------------------------------

def rgb_to_hsbk(r, g, b, kelvin=3500):
    """RGB (0..255) -> LIFX HSBK (each 0..65535, kelvin in K)."""
    h, s, v = colorsys.rgb_to_hsv(r / 255.0, g / 255.0, b / 255.0)
    return int(h * 65535), int(s * 65535), int(v * 65535), int(kelvin)


def hsbk_to_rgb(hue, sat, bri, kelvin=3500):
    """LIFX HSBK -> RGB (0..255). Kelvin is ignored for display purposes."""
    r, g, b = colorsys.hsv_to_rgb(hue / 65535.0, sat / 65535.0, bri / 65535.0)
    return int(r * 255), int(g * 255), int(b * 255)


# --------------------------------------------------------------------------
# Device model
# --------------------------------------------------------------------------

class Light:
    """A single discovered LIFX device and its last-known state."""

    def __init__(self, ip, target, label="", power=False, hsbk=None):
        self.ip = ip
        self.target = target            # 64-bit serial (MAC) used as address
        self.label = label
        self.power = power
        self.hsbk = hsbk or (0, 0, 0, 3500)

    @property
    def serial(self):
        """Human-readable serial, e.g. d073d5xxxxxx."""
        return self.target.to_bytes(8, "little")[:6].hex()

    @property
    def brightness_pct(self):
        return round(self.hsbk[2] / 65535 * 100)

    def __repr__(self):
        return f"<Light {self.label!r} {self.ip} {'on' if self.power else 'off'}>"


# --------------------------------------------------------------------------
# Controller
# --------------------------------------------------------------------------

class LifxLan:
    """Owns a UDP socket and drives discovery + commands."""

    def __init__(self, timeout=0.6):
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind(("", 0))
        self.sock.settimeout(timeout)
        self._seq = 0

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass

    def _next_seq(self):
        self._seq = (self._seq + 1) % 256
        return self._seq

    # -- discovery ---------------------------------------------------------

    @staticmethod
    def _local_ipv4s():
        """All local IPv4 interface addresses (minus loopback)."""
        ips = set()
        try:
            for info in socket.getaddrinfo(socket.gethostname(), None,
                                           socket.AF_INET):
                ips.add(info[4][0])
        except OSError:
            pass
        return [ip for ip in ips if not ip.startswith("127.")]

    def _discovery_sockets(self):
        """One broadcast socket bound to each interface, with its broadcast dests.

        Binding per-interface (rather than relying on the default route) is what
        makes discovery work when a VPN/WireGuard tunnel owns the default route -
        the 255.255.255.255 limited broadcast would otherwise leave via the VPN
        and never reach the LAN. We also send the subnet-directed broadcast.
        """
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
            socks.append((s, [directed, BROADCAST]))
        return socks

    def discover(self, wait=1.2, rounds=3):
        """Broadcast for devices and return a list of Lights with live state.

        UDP discovery is lossy and bulbs often miss the first request, so we run
        several rounds and accumulate every device that answers in any of them.
        We broadcast from every local interface so the search still works with a
        VPN active (see _discovery_sockets).
        """
        socks = self._discovery_sockets()
        if not socks:                       # fallback to the shared socket
            socks = [(self.sock, [BROADCAST])]
        readers = [s for s, _ in socks]
        found = {}
        try:
            for _ in range(max(1, rounds)):
                for s, dests in socks:
                    pkt = _make_packet(MSG_GET_SERVICE, tagged=True,
                                       sequence=self._next_seq())
                    for dest in dests:
                        try:
                            s.sendto(pkt, (dest, LIFX_PORT))
                        except OSError:
                            pass
                deadline = time.time() + wait
                while time.time() < deadline:
                    ready, _, _ = select.select(readers, [], [], 0.2)
                    for s in ready:
                        try:
                            data, addr = s.recvfrom(1024)
                        except OSError:
                            continue
                        _s, _src, target, _seq, mtype = _parse_header(data)
                        if mtype == MSG_STATE_SERVICE and len(data) >= 41:
                            service, _port = struct.unpack_from("<BI", data, 36)
                            if service == 1 and target not in found:   # 1 = UDP
                                found[target] = Light(addr[0], target)
        finally:
            for s, _ in socks:
                if s is not self.sock:
                    try:
                        s.close()
                    except OSError:
                        pass

        lights = list(found.values())
        for light in lights:
            self.refresh(light)
        lights.sort(key=lambda l: l.label.lower())
        return lights

    # -- low-level send ----------------------------------------------------

    def _send(self, light, msg_type, payload=b"", res_type=None,
              retries=3, sends=2):
        """Send a packet. If res_type is set, wait for a matching reply."""
        seq = self._next_seq()
        pkt = _make_packet(msg_type, payload, target=light.target,
                           res_required=bool(res_type), sequence=seq)

        if res_type is None:
            # Fire-and-forget: send a couple of times to survive UDP loss.
            for _ in range(sends):
                self.sock.sendto(pkt, (light.ip, LIFX_PORT))
            return None

        for _ in range(retries):
            self.sock.sendto(pkt, (light.ip, LIFX_PORT))
            deadline = time.time() + self.sock.gettimeout()
            while time.time() < deadline:
                try:
                    data, _addr = self.sock.recvfrom(1024)
                except socket.timeout:
                    break
                _s, _src, target, rseq, mtype = _parse_header(data)
                if mtype == res_type and rseq == seq and target == light.target:
                    return data
        return None

    # -- reads -------------------------------------------------------------

    def refresh(self, light):
        """Re-read the light's live state into the Light object."""
        data = self._send(light, MSG_GET, res_type=MSG_STATE)
        if not data or len(data) < 52:
            return False
        hue, sat, bri, kelvin, _res, power = struct.unpack_from("<HHHHhH", data, 36)
        label = data[48:80].split(b"\x00")[0].decode("utf-8", "replace")
        light.hsbk = (hue, sat, bri, kelvin)
        if label:
            light.label = label
        light.power = power > 0
        return True

    # -- writes ------------------------------------------------------------

    def set_power(self, light, on, duration_ms=400):
        payload = struct.pack("<HI", 65535 if on else 0, duration_ms)
        self._send(light, MSG_SET_POWER, payload)
        light.power = bool(on)

    def set_color(self, light, hue, sat, bri, kelvin, duration_ms=400):
        kelvin = max(KELVIN_MIN, min(KELVIN_MAX, int(kelvin)))
        payload = struct.pack("<BHHHHI", 0,
                              hue & 0xFFFF, sat & 0xFFFF, bri & 0xFFFF,
                              kelvin, duration_ms)
        self._send(light, MSG_SET_COLOR, payload)
        light.hsbk = (hue & 0xFFFF, sat & 0xFFFF, bri & 0xFFFF, kelvin)

    def set_brightness(self, light, pct, duration_ms=300):
        """Change brightness only, keeping current hue/sat/kelvin."""
        hue, sat, _bri, kelvin = light.hsbk
        bri = int(max(0, min(100, pct)) / 100 * 65535)
        self.set_color(light, hue, sat, bri, kelvin, duration_ms)

    def set_rgb(self, light, r, g, b, duration_ms=400, keep_brightness=True):
        """Set a color from RGB. By default keeps the current brightness."""
        hue, sat, bri, _k = rgb_to_hsbk(r, g, b)
        if keep_brightness:
            bri = light.hsbk[2]
        self.set_color(light, hue, sat, bri, light.hsbk[3], duration_ms)

    def set_white(self, light, pct, kelvin, duration_ms=400):
        """Set white at a colour temperature (saturation 0)."""
        bri = int(max(0, min(100, pct)) / 100 * 65535)
        self.set_color(light, 0, 0, bri, kelvin, duration_ms)


# --------------------------------------------------------------------------
# CLI smoke test:  python lifx_lan.py
# --------------------------------------------------------------------------

if __name__ == "__main__":
    lan = LifxLan()
    print("Discovering LIFX devices on the LAN ...")
    bulbs = lan.discover()
    if not bulbs:
        print("No devices found. Make sure the bulbs are powered and on the "
              "same network/subnet as this PC (UDP broadcast must reach them).")
    for b in bulbs:
        r, g, bl = hsbk_to_rgb(*b.hsbk)
        print(f"  {b.label:<20} {b.ip:<15} {b.serial}  "
              f"{'ON ' if b.power else 'OFF'}  {b.brightness_pct:>3}%  "
              f"rgb=({r},{g},{bl}) kelvin={b.hsbk[3]}")
    lan.close()
